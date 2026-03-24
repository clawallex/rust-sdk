# clawallex-sdk (Rust)

Async Rust SDK for the Clawallex Payment API.

## Dependencies

```toml
[dependencies]
clawallex-sdk = "1.0.0"
```

Requires `tokio` as your async runtime.

## Quick Start

```rust
use clawallex_sdk::{Client, Options};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // First run — SDK auto-resolves client_id via whoami/bootstrap
    let client = Client::new(Options {
        api_key: "your-api-key".into(),
        api_secret: "your-api-secret".into(),
        base_url: "https://api.clawallex.com".into(),
        client_id: None,
    }).await?;

    // ⬇️ Persist client.client_id to your config/database/env
    // e.g. "ca_8f0d2c3e5a1b4c7d"
    println!("{}", client.client_id);

    // Subsequent runs — pass the stored client_id to skip network calls
    let client = Client::new(Options {
        api_key: "your-api-key".into(),
        api_secret: "your-api-secret".into(),
        base_url: "https://api.clawallex.com".into(),
        client_id: Some("ca_8f0d2c3e5a1b4c7d".into()), // the value you persisted
    }).await?;

    Ok(())
}
```

## Client ID

`client_id` is your application's stable identity on Clawallex, separate from the API Key.

- You can rotate API Keys (revoke old, create new) without losing access to existing cards and transactions — just keep using the same `client_id`
- When a new API Key sends its first request with an existing `client_id`, the server auto-binds the new key to that identity
- Once bound, a `client_id` cannot be changed for that API Key (TOFU — Trust On First Use)
- Cards and transactions are isolated by `client_id` — different `client_id`s cannot see each other's data
- Wallet balance is shared at the user level (across all `client_id`s under the same user)

### Resolution

If `client_id` is provided at initialization, the SDK uses it directly (no network calls). If omitted, the SDK calls `GET /auth/whoami` — if already bound, uses the existing `client_id`; if not, calls `POST /auth/bootstrap` to generate and bind a new one.

### Best Practice

Persist the resolved `client_id` after the first initialization and pass it explicitly on subsequent sessions. This avoids unnecessary network calls and ensures identity continuity across API Key rotations.

### Data Isolation

| Scope | Isolation Level |
|-------|----------------|
| Wallet balance | User-level — shared across all `client_id`s under the same user |
| Cards | `client_id`-scoped — only visible to the `client_id` that created them |
| Transactions | `client_id`-scoped — only visible to the `client_id` that owns the card |
| Recharge addresses | User-level — shared |

## API

```rust
// Wallet
client.wallet_detail().await?;
client.recharge_addresses(&wallet_id).await?;

// X402 — chain_code is Option<&str>, defaults to "ETH" when None
client.x402_payee_address("USDC", None).await?;          // ETH chain
client.x402_asset_address("USDC", Some("BASE")).await?;  // explicit chain

// Cards
client.new_card(&params).await?;
client.card_list(CardListParams::default()).await?;
client.card_balance(&card_id).await?;
client.card_details(&card_id).await?;

// Transactions
client.transaction_list(TransactionListParams::default()).await?;

// Refill
client.refill_card(&card_id, &params).await?;
```

## Mode A — Wallet Funded Card

Mode A is the simplest path: cards are paid from your Clawallex wallet balance. No blockchain interaction needed.

### Create a Card

```rust
use clawallex_sdk::{mode_code, card_type};

let order = client.new_card(&NewCardParams {
    mode_code: mode_code::WALLET,                // Mode A
    card_type: card_type::FLASH,                 // FLASH or STREAM
    amount: "50.0000".into(),                    // card face value in USD
    client_request_id: uuid::Uuid::new_v4().to_string(),
    ..Default::default()
}).await?;

// order.card_order_id — always present
// order.card_id       — Some if card created synchronously
// order.status        — 200=active, 120=pending_async
```

### Handling Async Card Creation (status=120)

Card creation may be asynchronous — the issuer accepts the request but hasn't finished yet. **This is normal**, not an error. The wallet has already been charged.

```rust
let card_id = if let Some(id) = order.card_id {
    id
} else {
    // Poll card list until the new card appears
    let before = client.card_list(CardListParams {
        page: Some(1), page_size: Some(100),
    }).await?;
    let existing: std::collections::HashSet<String> =
        before.data.iter().map(|c| c.card_id.clone()).collect();

    let mut found = String::new();
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let list = client.card_list(CardListParams {
            page: Some(1), page_size: Some(100),
        }).await?;
        if let Some(card) = list.data.iter().find(|c| !existing.contains(&c.card_id)) {
            found = card.card_id.clone();
            break;
        }
    }
    found
};
```

> **Tip**: You can also retry `new_card` with the same `client_request_id`. The server will safely retry the issuer call without re-charging your wallet.

### Mode A Refill

```rust
let refill = client.refill_card(&card_id, &RefillCardParams {
    amount: "30.0000".into(),
    client_request_id: Some(uuid::Uuid::new_v4().to_string()),
    ..Default::default()
}).await?;
```

## Fee Structure

Fees are calculated server-side. For Mode B, the 402 response breaks them down:

| Fee field | Applies to | Description |
|-----------|-----------|-------------|
| `issue_fee_amount` | All cards | One-time card issuance fee |
| `monthly_fee_amount` | Stream cards only | First month fee (included in initial charge) |
| `fx_fee_amount` | All cards | Foreign exchange fee |
| `fee_amount` | — | `= issue_fee_amount + monthly_fee_amount + fx_fee_amount` |
| `payable_amount` | — | `= amount + fee_amount` (total to pay) |

- Flash cards: `fee_amount = issue_fee + fx_fee`
- Stream cards: `fee_amount = issue_fee + monthly_fee + fx_fee`
- Mode A refill: **no fees** — the refill amount goes directly to the card
- Mode B refill: **no fees** — same as Mode A

## Mode B — x402 On-Chain Payment (Two-Step)

Mode B is for Agents that hold their own wallet and private key. The card is funded by an on-chain USDC transfer via the EIP-3009 `transferWithAuthorization` standard — no human intervention needed.

> **Mode B currently only supports USDC** (6 decimals) on ETH and BASE chains. `token_code` must be `"USDC"`.

### Flow

```
Agent → POST /card-orders (mode_code=200)     → 402 + quote details
Agent → sign EIP-3009 with private key
Agent → POST /card-orders (same client_request_id) → 200 + card created
```

### Stage 1 — Request Quote (402 is expected, not an error)

```rust
use clawallex_sdk::{Client, NewCardParams, Error, CardOrder402Details, mode_code, card_type};
use uuid::Uuid;

let client_request_id = Uuid::new_v4().to_string();
let mut details: Option<CardOrder402Details> = None;

match client.new_card(&NewCardParams {
    mode_code: mode_code::X402,
    card_type: card_type::STREAM,  // FLASH or STREAM
    amount: "200.0000".into(),
    client_request_id: client_request_id.clone(),
    chain_code: Some("ETH".into()),   // or "BASE"
    token_code: Some("USDC".into()),
    ..Default::default()
}).await {
    Err(Error::PaymentRequired { details: d, .. }) => {
        // d.payee_address    — system receiving address
        // d.asset_address    — USDC contract address
        // d.payable_amount   — total including fees (e.g. "207.5900")
        // d.x402_reference_id — must be echoed in Stage 2
        // d.final_card_amount, d.fee_amount,
        // d.issue_fee_amount, d.monthly_fee_amount, d.fx_fee_amount
        details = Some(d);
    }
    other => { other?; }
}
let details = details.unwrap();
```

### EIP-3009 Signing (using ethers-rs or alloy)

```rust
use alloy_primitives::{B256, U256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{eip712_domain, sol, SolStruct};
use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

let signer: PrivateKeySigner = PRIVATE_KEY.parse()?;
let from_address = signer.address();

let payable: f64 = details.payable_amount.parse()?;
let max_amount_required = ((payable * 1_000_000.0).floor()) as u64;
let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
let nonce: B256 = rand::thread_rng().gen::<[u8; 32]>().into();

// Define the EIP-712 domain and struct with alloy's sol! macro
sol! {
    #[derive(Default)]
    struct TransferWithAuthorization {
        address from;
        address to;
        uint256 value;
        uint256 validAfter;
        uint256 validBefore;
        bytes32 nonce;
    }
}

// Domain name is "USDC" on Sepolia; may be "USD Coin" on mainnet.
// chainId: 11155111 (Sepolia), 1 (ETH mainnet), 8453 (BASE)
let domain = eip712_domain! {
    name: "USDC",
    version: "2",
    chain_id: 11155111,
    verifying_contract: details.asset_address.parse()?,
};

let message = TransferWithAuthorization {
    from: from_address,
    to: details.payee_address.parse()?,
    value: U256::from(max_amount_required),
    validAfter: U256::from(now - 60),
    validBefore: U256::from(now + 3600),
    nonce,
};

let hash = message.eip712_signing_hash(&domain);
let signature = signer.sign_hash_sync(&hash)?;
let sig_hex = format!("0x{}", hex::encode(signature.as_bytes()));
```

> **Note**: The EIP-712 domain `name` depends on the USDC contract deployment.
> On Sepolia testnet it is `"USDC"`, on mainnet it may be `"USD Coin"`.
> Query the contract's `name()` method to confirm.

### Stage 2 — Submit Payment

> **IMPORTANT**: Stage 2 **must** use the same `client_request_id` as Stage 1.
> A different `client_request_id` will create a **new** card order instead of completing the current one.

The SDK provides typed structs `X402Authorization`, `X402PaymentPayload`, and `X402PaymentRequirements` with a `.to_value()` method that returns a `serde_json::Value`:

```rust
use clawallex::{X402Authorization, X402PaymentPayload, X402PaymentRequirements};

let authorization = X402Authorization {
    from: from_address.to_string(),
    to: details.payee_address.clone(),
    value: max_amount_required.to_string(),
    valid_after: (now - 60).to_string(),
    valid_before: (now + 3600).to_string(),
    nonce: format!("0x{}", hex::encode(nonce)),
};

let payload = X402PaymentPayload {
    scheme: "exact".into(),
    network: "ETH".into(),
    signature: sig_hex,
    authorization,
};

let requirements = X402PaymentRequirements {
    scheme: "exact".into(),
    network: "ETH".into(),                          // must equal payload.network
    asset: details.asset_address.clone(),           // must equal 402 asset_address
    pay_to: details.payee_address.clone(),          // must equal authorization.to
    max_amount_required: max_amount_required.to_string(), // must equal authorization.value
    reference_id: details.x402_reference_id.clone(),
};

let order = client.new_card(&NewCardParams {
    mode_code: mode_code::X402,
    card_type: card_type::STREAM,
    amount: "200.0000".into(),
    client_request_id: client_request_id,            // MUST reuse from Stage 1
    x402_version: Some(1),
    payment_payload: Some(payload.to_value()),
    payment_requirements: Some(requirements.to_value()),
    extra: Some([
        ("card_amount".into(), details.final_card_amount.clone()),
        ("paid_amount".into(), details.payable_amount.clone()),
    ].into()),
    payer_address: Some(from_address.to_string()),
    ..Default::default()
}).await?;
// order.card_order_id, order.card_id, order.status
```

### Mode B Refill (No 402 — Direct Submit)

Refill has **no 402 challenge**. Query addresses first, then submit directly:

```rust
// 1. query addresses
let payee = client.x402_payee_address("USDC", None).await?;          // defaults to ETH
let asset = client.x402_asset_address("USDC", Some("ETH")).await?;

// 2. sign EIP-3009 (same as above, but amount has no fee)
let refill_amount = "30.0000";
let max_amt = ((30.0000_f64 * 1_000_000.0).floor()) as u64;
// ... sign with signer ...

// 3. submit refill
let refill = client.refill_card(&card_id, &RefillCardParams {
    amount: refill_amount.into(),
    x402_reference_id: Some(Uuid::new_v4().to_string()),  // unique per refill
    x402_version: Some(1),
    payment_payload: Some(payload.to_value()),
    payment_requirements: Some(requirements.to_value()),
    payer_address: Some(from_address.to_string()),
    ..Default::default()
}).await?;
```

### Consistency Rules (Server Rejects if Any Fail)

| # | Rule |
|---|------|
| 1 | `payment_payload.network` == `payment_requirements.network` |
| 2 | `authorization.to` == `payTo` == 402 `payee_address` |
| 3 | `authorization.value` == `maxAmountRequired` == `payable_amount × 10^6` |
| 4 | `payment_requirements.asset` == 402 `asset_address` |
| 5 | `extra.referenceId` == 402 `x402_reference_id` |
| 6 | `extra.card_amount` == original `amount` |
| 7 | `extra.paid_amount` == 402 `payable_amount` |

## Card Details — Decrypting PAN/CVV

`card_details` returns encrypted sensitive data. The server encrypts with a key derived from your `api_secret`.

```rust
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::{Engine, engine::general_purpose::STANDARD};
use hkdf::Hkdf;
use sha2::Sha256;

let details = client.card_details(&card_id).await?;
let enc = &details.encrypted_sensitive_data;
// enc.version = "v1", enc.algorithm = "AES-256-GCM", enc.kdf = "HKDF-SHA256"

// 1. Derive 32-byte key from api_secret using HKDF-SHA256
let hk = Hkdf::<Sha256>::new(None, api_secret.as_bytes());
let mut derived_key = [0u8; 32];
hk.expand(b"clawallex-card-sensitive-data", &mut derived_key).unwrap();

// 2. Decrypt with AES-256-GCM
let nonce_bytes = STANDARD.decode(&enc.nonce).unwrap();
let ciphertext = STANDARD.decode(&enc.ciphertext).unwrap();

let cipher = Aes256Gcm::new_from_slice(&derived_key).unwrap();
let nonce = Nonce::from_slice(&nonce_bytes);
let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).unwrap();

let card_data: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
let pan = card_data["pan"].as_str().unwrap();  // "4111111111111111"
let cvv = card_data["cvv"].as_str().unwrap();  // "123"
```

> **Security**: Never log or persist the decrypted PAN/CVV in plaintext. The `api_secret` must be at least 16 bytes. Add `aes-gcm` and `hkdf` crates to your dependencies.

## Error Handling

```rust
use clawallex::Error;

match client.new_card(&params).await {
    Ok(order) => println!("{}", order.card_order_id),
    Err(Error::PaymentRequired { details, .. }) => {
        // Mode B challenge — normal first-request flow
        println!("pay to: {}", details.payee_address);
    }
    Err(Error::Api { status, code, message }) => {
        eprintln!("{status} {code}: {message}");
    }
    Err(e) => return Err(e.into()),
}
```

## Enums Reference

| Constant | Named Constant | Value | Description |
|----------|---------------|-------|-------------|
| `mode_code` | `mode_code::WALLET` | `100` | Mode A — wallet funded |
| `mode_code` | `mode_code::X402` | `200` | Mode B — x402 on-chain |
| `card_type` | `card_type::FLASH` | `100` | Flash card |
| `card_type` | `card_type::STREAM` | `200` | Stream card (subscription) |
| `card.status` | `200` | Active |
| `card.status` | `220` | Closing |
| `card.status` | `230` | Expired |
| `card.status` | `250` | Cancelled |
| `wallet.status` | `100` | Normal |
| `wallet.status` | `210` | Frozen |
