//! Clawallex Payment API SDK

use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Funding source for card creation.
pub mod mode_code {
    /// Mode A: deduct from wallet balance.
    pub const WALLET: i32 = 100;
    /// Mode B: on-chain x402 USDC payment.
    pub const X402: i32 = 200;
}

/// Card lifecycle.
pub mod card_type {
    /// One-time use, auto-destroyed after a single transaction.
    pub const FLASH: i32 = 100;
    /// Reloadable, suitable for recurring payments.
    pub const STREAM: i32 = 200;
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Payment challenge returned with HTTP 402 during a Mode B card order.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CardOrder402Details {
    pub card_order_id: String,
    pub client_request_id: String,
    pub x402_reference_id: String,
    pub payee_address: String,
    pub asset_address: String,
    pub final_card_amount: String,
    pub issue_fee_amount: String,
    pub monthly_fee_amount: String,
    pub fx_fee_amount: String,
    pub fee_amount: String,
    pub payable_amount: String,
}

#[derive(Debug)]
pub enum Error {
    /// HTTP 402 — Mode B payment challenge. Normal first-request flow.
    PaymentRequired {
        code: String,
        message: String,
        details: CardOrder402Details,
    },
    /// Non-2xx API response.
    Api {
        status: u16,
        code: String,
        message: String,
    },
    Http(reqwest::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PaymentRequired { code, message, .. } => {
                write!(f, "clawallex: 402 {code} — {message}")
            }
            Error::Api { status, code, message } => {
                write!(f, "clawallex: {status} {code} — {message}")
            }
            Error::Http(e) => write!(f, "clawallex: http: {e}"),
            Error::Json(e) => write!(f, "clawallex: json: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct WalletDetail {
    pub wallet_id: String,
    pub wallet_type: i32,
    pub currency: String,
    pub available_balance: String,
    pub frozen_balance: String,
    pub low_balance_threshold: String,
    pub status: i32,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RechargeAddress {
    pub recharge_address_id: String,
    pub wallet_id: String,
    pub chain_code: String,
    pub token_code: String,
    pub address: String,
    pub memo_tag: String,
    pub status: i32,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RechargeAddressesResponse {
    pub wallet_id: String,
    pub total: i32,
    pub data: Vec<RechargeAddress>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PayeeAddressResponse {
    pub chain_code: String,
    pub token_code: String,
    pub address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetAddressResponse {
    pub chain_code: String,
    pub token_code: String,
    pub asset_address: String,
}

// ─── x402 / EIP-3009 payload types ───────────────────────────────────────────

/// EIP-3009 `transferWithAuthorization` fields.
///
/// See <https://eips.ethereum.org/EIPS/eip-3009>
#[derive(Debug, Clone, Serialize)]
pub struct X402Authorization {
    /// Agent wallet address (payer)
    pub from: String,
    /// Must equal 402 `payee_address`
    pub to: String,
    /// `payable_amount × 10^decimals` (USDC=6, e.g. `"207590000"`)
    pub value: String,
    /// Unix seconds, recommended `now - 60`
    #[serde(rename = "validAfter")]
    pub valid_after: String,
    /// Unix seconds, recommended `now + 3600`
    #[serde(rename = "validBefore")]
    pub valid_before: String,
    /// Random 32-byte hex with `0x` prefix
    pub nonce: String,
}

/// x402 payment payload — wraps the EIP-3009 signature + authorization.
#[derive(Debug, Clone, Serialize)]
pub struct X402PaymentPayload {
    /// Fixed `"exact"`
    pub scheme: String,
    /// Chain network: `"ETH"` / `"BASE"`
    pub network: String,
    pub payload: X402PaymentPayloadInner,
}

#[derive(Debug, Clone, Serialize)]
pub struct X402PaymentPayloadInner {
    /// EIP-3009 typed-data signature hex
    pub signature: String,
    pub authorization: X402Authorization,
}

/// x402 payment requirements — describes what the payment must satisfy.
#[derive(Debug, Clone, Serialize)]
pub struct X402PaymentRequirements {
    /// Fixed `"exact"`
    pub scheme: String,
    /// Must equal `payment_payload.network`
    pub network: String,
    /// Token contract address — must equal 402 `asset_address`
    pub asset: String,
    /// Must equal 402 `payee_address` and `authorization.to`
    #[serde(rename = "payTo")]
    pub pay_to: String,
    /// Must equal `authorization.value`
    #[serde(rename = "maxAmountRequired")]
    pub max_amount_required: String,
    pub extra: X402RequirementsExtra,
}

#[derive(Debug, Clone, Serialize)]
pub struct X402RequirementsExtra {
    /// Must equal 402 `x402_reference_id`
    #[serde(rename = "referenceId")]
    pub reference_id: String,
}

impl X402PaymentPayload {
    /// Convert to `serde_json::Value` for use in `NewCardParams.payment_payload`.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

impl X402PaymentRequirements {
    /// Convert to `serde_json::Value` for use in `NewCardParams.payment_requirements`.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
}

// ─── Cards ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct NewCardParams {
    pub mode_code: i32,
    pub card_type: i32,
    pub amount: String,
    pub client_request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer_card_currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_mcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x402_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x402_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_requirements: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<std::collections::HashMap<String, String>>,
    /// Card TTL in seconds. Flash cards only; omit for the default 24-hour expiry.
    /// Sets issuer `expiry_at = now + ttl`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_address: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardOrderResponse {
    pub card_order_id: String,
    pub status: i32,
    pub card_id: Option<String>,
    pub reference_id: Option<String>,
    pub idempotent: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct CardListParams {
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Card {
    pub card_id: String,
    pub mode_code: i32,
    pub card_type: i32,
    pub status: i32,
    pub masked_pan: String,
    pub card_currency: String,
    pub available_balance: String,
    pub expiry_month: i32,
    pub expiry_year: i32,
    pub issuer_card_status: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardListResponse {
    pub total: i32,
    pub page: i32,
    pub page_size: i32,
    pub data: Vec<Card>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardBalanceResponse {
    pub card_id: String,
    pub card_currency: String,
    pub available_balance: String,
    pub status: i32,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EncryptedSensitiveData {
    pub version: String,
    pub algorithm: String,
    pub kdf: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardDetailsResponse {
    pub card_id: String,
    pub masked_pan: String,
    pub encrypted_sensitive_data: EncryptedSensitiveData,
    pub expiry_month: i32,
    pub expiry_year: i32,
    pub tx_limit: String,
    pub allowed_mcc: String,
    pub blocked_mcc: String,
    pub card_currency: String,
    pub available_balance: String,
    pub first_name: String,
    pub last_name: String,
    /// Billing address — JSON string or plain text
    pub delivery_address: String,
    pub status: i32,
    pub issuer_card_status: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct TransactionListParams {
    pub card_tx_id: Option<String>,
    pub issuer_tx_id: Option<String>,
    pub card_id: Option<String>,
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transaction {
    pub card_id: String,
    pub card_tx_id: String,
    pub issuer_tx_id: String,
    pub issuer_ori_tx_id: String,
    pub action_type: i32,
    pub tx_type: i32,
    pub process_status: String,
    pub amount: String,
    pub fee_amount: String,
    pub fee_currency: String,
    pub billing_amount: String,
    pub billing_currency: String,
    pub transaction_amount: String,
    pub transaction_currency: String,
    pub status: i32,
    pub card_fund_applied: i32,
    pub is_in_progress: i32,
    pub merchant_name: String,
    pub mcc: String,
    pub decline_reason: String,
    pub description: String,
    pub issuer_card_available_balance: String,
    pub occurred_at: String,
    pub settled_at: Option<String>,
    pub webhook_event_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransactionListResponse {
    pub card_tx_id: String,
    pub issuer_tx_id: String,
    pub card_id: String,
    pub page: i32,
    pub page_size: i32,
    pub total: i32,
    pub data: Vec<Transaction>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UpdateCardParams {
    pub client_request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_mcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_mcc: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateCardResponse {
    pub card_id: String,
    pub card_order_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchCardBalanceResponse {
    pub data: Vec<CardBalanceResponse>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RefillCardParams {
    pub amount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x402_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x402_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_requirements: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_address: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RefillResponse {
    pub card_id: String,
    pub refill_order_id: String,
    pub refilled_amount: String,
    pub status: String,
    pub related_transfer_id: Option<String>,
    pub x402_payment_id: Option<String>,
}

// ─── Options ──────────────────────────────────────────────────────────────────

pub struct Options {
    pub api_key: String,
    pub api_secret: String,
    pub base_url: String,
    /// If provided, skips whoami/bootstrap and uses this client_id directly.
    pub client_id: Option<String>,
}

// ─── Client ───────────────────────────────────────────────────────────────────

const BASE_PATH: &str = "/api/v1";

pub struct Client {
    opts: Options,
    pub client_id: String,
    http: reqwest::Client,
}

impl Client {
    /// Create a fully initialised client.
    ///
    /// - If `opts.client_id` is `Some`, it is used directly.
    /// - Otherwise calls `GET /auth/whoami`; if already bound uses the existing
    ///   `bound_client_id`, else calls `POST /auth/bootstrap` to obtain one.
    pub async fn new(opts: Options) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        if let Some(ref id) = opts.client_id {
            return Ok(Client { client_id: id.clone(), opts, http });
        }

        // Temporary client with empty client_id for /auth/* calls
        let tmp = Client { client_id: String::new(), opts, http };

        #[derive(Deserialize)]
        struct WhoamiResp {
            client_id_bound: bool,
            bound_client_id: String,
        }
        let whoami: WhoamiResp = tmp.do_request("GET", "/auth/whoami", "", None::<&()>, false).await?;

        let client_id = if whoami.client_id_bound {
            whoami.bound_client_id
        } else {
            #[derive(Deserialize)]
            struct BootstrapResp { client_id: String }
            let b: BootstrapResp = tmp.do_request("POST", "/auth/bootstrap", "", Some(&serde_json::json!({})), false).await?;
            b.client_id
        };

        Ok(Client { client_id, ..tmp })
    }

    fn sign(&self, method: &str, path: &str, body: &str, include_client_id: bool) -> Vec<(String, String)> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();
        let body_hash = hex::encode(Sha256::digest(body.as_bytes()));
        let canonical = format!("{method}\n{path}\n{ts}\n{body_hash}");
        let mut mac = HmacSha256::new_from_slice(self.opts.api_secret.as_bytes()).unwrap();
        mac.update(canonical.as_bytes());
        let sig = BASE64.encode(mac.finalize().into_bytes());

        let mut headers = vec![
            ("X-API-Key".into(), self.opts.api_key.clone()),
            ("X-Timestamp".into(), ts),
            ("X-Signature".into(), sig),
            ("Content-Type".into(), "application/json".into()),
        ];
        if include_client_id {
            headers.push(("X-Client-Id".into(), self.client_id.clone()));
        }
        headers
    }

    async fn do_request<B, T>(&self, method: &str, path: &str, query: &str, body: Option<&B>, include_client_id: bool) -> Result<T, Error>
    where
        B: Serialize,
        T: DeserializeOwned,
    {
        let full_path = format!("{BASE_PATH}{path}");
        let url = if query.is_empty() {
            format!("{}{}", self.opts.base_url.trim_end_matches('/'), full_path)
        } else {
            format!("{}{}?{}", self.opts.base_url.trim_end_matches('/'), full_path, query)
        };

        let raw_body = match body {
            Some(b) => serde_json::to_string(b)?,
            None => String::new(),
        };

        let mut req = self.http.request(method.parse().unwrap(), &url);
        for (k, v) in self.sign(method, &full_path, &raw_body, include_client_id) {
            req = req.header(k, v);
        }
        if !raw_body.is_empty() {
            req = req.body(raw_body);
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;

        if status == 402 {
            #[derive(Deserialize)]
            struct Envelope {
                #[serde(default)]
                code: String,
                #[serde(default)]
                message: String,
                #[serde(default)]
                details: Option<CardOrder402Details>,
            }
            let env: Envelope = serde_json::from_str(&text).unwrap_or(Envelope {
                code: "PAYMENT_REQUIRED".into(),
                message: "Payment required".into(),
                details: None,
            });
            return Err(Error::PaymentRequired {
                code: if env.code.is_empty() { "PAYMENT_REQUIRED".into() } else { env.code },
                message: env.message,
                details: env.details.unwrap_or_default(),
            });
        }

        if status < 200 || status >= 300 {
            #[derive(Deserialize, Default)]
            struct ApiErr {
                #[serde(default)]
                code: String,
                #[serde(default)]
                message: String,
            }
            let e: ApiErr = serde_json::from_str(&text).unwrap_or_default();
            return Err(Error::Api {
                status,
                code: if e.code.is_empty() { "UNKNOWN_ERROR".into() } else { e.code },
                message: if e.message.is_empty() { text } else { e.message },
            });
        }

        Ok(serde_json::from_str(&text)?)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T, Error> {
        let qs: String = query
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        self.do_request::<(), T>("GET", path, &qs, None, true).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, Error> {
        self.do_request("POST", path, "", Some(body), true).await
    }

    // ── Wallet ────────────────────────────────────────────────────────────────

    pub async fn wallet_detail(&self) -> Result<WalletDetail, Error> {
        self.get("/payment/wallets/detail", &[]).await
    }

    pub async fn recharge_addresses(&self, wallet_id: &str) -> Result<RechargeAddressesResponse, Error> {
        self.get(&format!("/payment/wallets/{wallet_id}/recharge-addresses"), &[]).await
    }

    // ── X402 ──────────────────────────────────────────────────────────────────

    /// `chain_code` defaults to `"ETH"` if `None`.
    pub async fn x402_payee_address(&self, token_code: &str, chain_code: Option<&str>) -> Result<PayeeAddressResponse, Error> {
        let chain = chain_code.unwrap_or("ETH");
        self.get("/payment/x402/payee-address", &[("chain_code", chain), ("token_code", token_code)]).await
    }

    /// `chain_code` defaults to `"ETH"` if `None`.
    pub async fn x402_asset_address(&self, token_code: &str, chain_code: Option<&str>) -> Result<AssetAddressResponse, Error> {
        let chain = chain_code.unwrap_or("ETH");
        self.get("/payment/x402/asset-address", &[("chain_code", chain), ("token_code", token_code)]).await
    }

    // ── Cards ─────────────────────────────────────────────────────────────────

    /// Create a card order.
    ///
    /// For Mode B, the first call returns `Err(Error::PaymentRequired { details, .. })`.
    /// Read `details` to build the x402 payment, then call again with the same
    /// `client_request_id` and the payment fields populated.
    pub async fn new_card(&self, params: &NewCardParams) -> Result<CardOrderResponse, Error> {
        self.post("/payment/card-orders", params).await
    }

    pub async fn card_list(&self, params: CardListParams) -> Result<CardListResponse, Error> {
        let page = params.page.map(|p| p.to_string()).unwrap_or_default();
        let page_size = params.page_size.map(|p| p.to_string()).unwrap_or_default();
        self.get("/payment/cards", &[("page", &page), ("page_size", &page_size)]).await
    }

    pub async fn card_balance(&self, card_id: &str) -> Result<CardBalanceResponse, Error> {
        self.get(&format!("/payment/cards/{card_id}/balance"), &[]).await
    }

    pub async fn card_details(&self, card_id: &str) -> Result<CardDetailsResponse, Error> {
        self.get(&format!("/payment/cards/{card_id}/details"), &[]).await
    }

    pub async fn batch_card_balances(&self, card_ids: &[&str]) -> Result<BatchCardBalanceResponse, Error> {
        self.post("/payment/cards/balances", &serde_json::json!({"card_ids": card_ids})).await
    }

    pub async fn update_card(&self, card_id: &str, params: &UpdateCardParams) -> Result<UpdateCardResponse, Error> {
        self.post(&format!("/payment/cards/{card_id}/update"), params).await
    }

    // ── Transactions ──────────────────────────────────────────────────────────

    pub async fn transaction_list(&self, params: TransactionListParams) -> Result<TransactionListResponse, Error> {
        let page = params.page.map(|p| p.to_string()).unwrap_or_default();
        let page_size = params.page_size.map(|p| p.to_string()).unwrap_or_default();
        self.get(
            "/payment/transactions",
            &[
                ("card_tx_id",  params.card_tx_id.as_deref().unwrap_or("")),
                ("issuer_tx_id", params.issuer_tx_id.as_deref().unwrap_or("")),
                ("card_id",      params.card_id.as_deref().unwrap_or("")),
                ("page",        &page),
                ("page_size",   &page_size),
            ],
        )
        .await
    }

    // ── Refill ────────────────────────────────────────────────────────────────

    pub async fn refill_card(&self, card_id: &str, params: &RefillCardParams) -> Result<RefillResponse, Error> {
        self.post(&format!("/payment/cards/{card_id}/refill"), params).await
    }
}
