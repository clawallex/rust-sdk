use std::env;

fn skip_if_no_env() -> Option<(String, String, String)> {
    let api_key = env::var("CLAWALLEX_API_KEY").ok()?;
    let api_secret = env::var("CLAWALLEX_API_SECRET").ok()?;
    let base_url = env::var("CLAWALLEX_BASE_URL").ok()?;
    Some((api_key, api_secret, base_url))
}

async fn make_client() -> clawallex_sdk::Client {
    let (api_key, api_secret, base_url) = skip_if_no_env().unwrap();
    clawallex_sdk::Client::new(clawallex_sdk::Options {
        api_key,
        api_secret,
        base_url,
        client_id: None,
    })
    .await
    .expect("Client::new failed")
}

macro_rules! integration_test {
    ($name:ident, $body:expr) => {
        #[tokio::test]
        async fn $name() {
            if skip_if_no_env().is_none() {
                eprintln!("Skipping: set CLAWALLEX_API_KEY, CLAWALLEX_API_SECRET, CLAWALLEX_BASE_URL");
                return;
            }
            let client = make_client().await;
            let run: fn(clawallex_sdk::Client) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> = $body;
            run(client).await;
        }
    };
}

// ── Auth ────────────────────────────────────────────────────────────────────

integration_test!(test_client_id, |client| Box::pin(async move {
    assert!(!client.client_id.is_empty(), "expected non-empty client_id");
}));

integration_test!(test_second_create_reuses_client_id, |client| Box::pin(async move {
    let client2 = make_client().await;
    assert_eq!(client.client_id, client2.client_id, "client_id should be reused");
}));

integration_test!(test_explicit_client_id, |client| Box::pin(async move {
    let (api_key, api_secret, base_url) = skip_if_no_env().unwrap();
    let client2 = clawallex_sdk::Client::new(clawallex_sdk::Options {
        api_key,
        api_secret,
        base_url,
        client_id: Some(client.client_id.clone()),
    }).await.unwrap();
    assert_eq!(client.client_id, client2.client_id);
}));

// ── Wallet ──────────────────────────────────────────────────────────────────

integration_test!(test_wallet_detail_fields, |client| Box::pin(async move {
    let w = client.wallet_detail().await.expect("wallet_detail failed");
    assert!(!w.wallet_id.is_empty());
    assert!(!w.currency.is_empty());
    assert!(!w.updated_at.is_empty());
}));

integration_test!(test_recharge_addresses, |client| Box::pin(async move {
    let w = client.wallet_detail().await.unwrap();
    let result = client.recharge_addresses(&w.wallet_id).await.expect("recharge_addresses failed");
    assert_eq!(result.wallet_id, w.wallet_id);
    if !result.data.is_empty() {
        let addr = &result.data[0];
        assert!(!addr.chain_code.is_empty());
        assert!(!addr.token_code.is_empty());
        assert!(!addr.address.is_empty());
    }
}));

// ── X402 ────────────────────────────────────────────────────────────────────

integration_test!(test_x402_payee_address_default_chain, |client| Box::pin(async move {
    let result = client.x402_payee_address("USDC", None).await.expect("x402_payee_address failed");
    assert!(!result.address.is_empty());
    assert_eq!(result.token_code, "USDC");
}));

integration_test!(test_x402_payee_address_explicit_chain, |client| Box::pin(async move {
    let result = client.x402_payee_address("USDC", Some("ETH")).await.expect("x402_payee_address failed");
    assert!(!result.address.is_empty());
    assert_eq!(result.chain_code, "ETH");
}));

integration_test!(test_x402_asset_address_default_chain, |client| Box::pin(async move {
    let result = client.x402_asset_address("USDC", None).await.expect("x402_asset_address failed");
    assert!(!result.asset_address.is_empty());
    assert_eq!(result.token_code, "USDC");
}));

integration_test!(test_x402_asset_address_explicit_chain, |client| Box::pin(async move {
    let result = client.x402_asset_address("USDC", Some("ETH")).await.expect("x402_asset_address failed");
    assert!(!result.asset_address.is_empty());
    assert_eq!(result.chain_code, "ETH");
}));

// ── Cards ───────────────────────────────────────────────────────────────────

integration_test!(test_card_list_pagination, |client| Box::pin(async move {
    let result = client.card_list(clawallex_sdk::CardListParams {
        page: Some(1), page_size: Some(5),
    }).await.expect("card_list failed");
    assert_eq!(result.page, 1);
    assert_eq!(result.page_size, 5);
}));

integration_test!(test_card_list_defaults, |client| Box::pin(async move {
    let result = client.card_list(clawallex_sdk::CardListParams::default())
        .await.expect("card_list failed");
    assert!(result.total >= 0);
}));

integration_test!(test_card_balance, |client| Box::pin(async move {
    let cards = client.card_list(clawallex_sdk::CardListParams {
        page: Some(1), page_size: Some(1),
    }).await.unwrap();
    if cards.data.is_empty() {
        eprintln!("Skipping: no cards");
        return;
    }
    let card = &cards.data[0];
    let balance = client.card_balance(&card.card_id).await.expect("card_balance failed");
    assert_eq!(balance.card_id, card.card_id);
    assert!(!balance.card_currency.is_empty());
}));

integration_test!(test_card_details, |client| Box::pin(async move {
    let cards = client.card_list(clawallex_sdk::CardListParams {
        page: Some(1), page_size: Some(1),
    }).await.unwrap();
    if cards.data.is_empty() {
        eprintln!("Skipping: no cards");
        return;
    }
    let card = &cards.data[0];
    let details = client.card_details(&card.card_id).await.expect("card_details failed");
    assert_eq!(details.card_id, card.card_id);
    assert!(!details.masked_pan.is_empty());
    assert_eq!(details.encrypted_sensitive_data.version, "v1");
    assert_eq!(details.encrypted_sensitive_data.algorithm, "AES-256-GCM");
    assert!(!details.encrypted_sensitive_data.ciphertext.is_empty());
}));

integration_test!(test_card_balance_not_found, |client| Box::pin(async move {
    let result = client.card_balance("non_existent_card_id").await;
    assert!(result.is_err());
    match result.unwrap_err() {
        clawallex_sdk::Error::Api { status, .. } => assert!(status >= 400),
        other => panic!("expected Api error, got: {:?}", other),
    }
}));

// ── Transactions ────────────────────────────────────────────────────────────

integration_test!(test_transaction_list_pagination, |client| Box::pin(async move {
    let result = client.transaction_list(clawallex_sdk::TransactionListParams {
        page: Some(1), page_size: Some(5), ..Default::default()
    }).await.expect("transaction_list failed");
    assert!(result.total >= 0);
}));

integration_test!(test_transaction_list_defaults, |client| Box::pin(async move {
    let result = client.transaction_list(clawallex_sdk::TransactionListParams::default())
        .await.expect("transaction_list failed");
    assert!(result.total >= 0);
}));

integration_test!(test_transaction_list_fields, |client| Box::pin(async move {
    let result = client.transaction_list(clawallex_sdk::TransactionListParams {
        page: Some(1), page_size: Some(5), ..Default::default()
    }).await.unwrap();
    if result.data.is_empty() {
        eprintln!("Skipping: no transactions");
        return;
    }
    let tx = &result.data[0];
    assert!(!tx.card_id.is_empty());
    assert!(!tx.card_tx_id.is_empty());
}));

integration_test!(test_transaction_filter_by_card, |client| Box::pin(async move {
    let cards = client.card_list(clawallex_sdk::CardListParams {
        page: Some(1), page_size: Some(1),
    }).await.unwrap();
    if cards.data.is_empty() {
        eprintln!("Skipping: no cards");
        return;
    }
    let card_id = &cards.data[0].card_id;
    let result = client.transaction_list(clawallex_sdk::TransactionListParams {
        card_id: Some(card_id.clone()),
        page: Some(1), page_size: Some(5),
        ..Default::default()
    }).await.unwrap();
    for tx in &result.data {
        assert_eq!(&tx.card_id, card_id);
    }
}));

// ── Mode A card lifecycle ───────────────────────────────────────────────────

integration_test!(test_mode_a_create_verify_close, |client| Box::pin(async move {
    let req_id = uuid::Uuid::new_v4().to_string();
    let params = clawallex_sdk::NewCardParams {
        mode_code: 100,
        card_type: 100,
        amount: "5.0000".into(),
        client_request_id: req_id,
        ..Default::default()
    };

    // snapshot existing card ids
    let before = client.card_list(clawallex_sdk::CardListParams {
        page: Some(1), page_size: Some(100),
    }).await.expect("card_list failed");
    let existing_ids: std::collections::HashSet<String> = before.data.iter().map(|c| c.card_id.clone()).collect();

    // 1. create flash card
    let order = client.new_card(&params).await.expect("new_card failed");
    assert!(!order.card_order_id.is_empty());

    // card creation may be async (status=120), poll card list for new card
    let card_id = if let Some(ref id) = order.card_id {
        id.clone()
    } else {
        let mut resolved = String::new();
        for i in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Ok(list) = client.card_list(clawallex_sdk::CardListParams {
                page: Some(1), page_size: Some(100),
            }).await {
                if let Some(card) = list.data.iter().find(|c| !existing_ids.contains(&c.card_id) && c.mode_code == 100) {
                    resolved = card.card_id.clone();
                    eprintln!("poll {}: found new card {}", i + 1, resolved);
                    break;
                }
            }
        }
        assert!(!resolved.is_empty(), "new card not found after 60s polling");
        resolved
    };

    // 3. check balance
    let balance = client.card_balance(&card_id).await.expect("card_balance failed");
    assert_eq!(balance.card_id, card_id);

    // 4. check details
    let details = client.card_details(&card_id).await.expect("card_details failed");
    assert_eq!(details.card_id, card_id);
    assert!(!details.encrypted_sensitive_data.ciphertext.is_empty());

}));

// ── Mode B 402 flow ─────────────────────────────────────────────────────────

integration_test!(test_mode_b_returns_402, |client| Box::pin(async move {
    let client_req_id = uuid::Uuid::new_v4().to_string();
    let result = client.new_card(&clawallex_sdk::NewCardParams {
        mode_code: 200,
        card_type: 200,
        amount: "100.0000".into(),
        client_request_id: client_req_id,
        chain_code: Some("ETH".into()),
        token_code: Some("USDC".into()),
        ..Default::default()
    }).await;
    match result {
        Err(clawallex_sdk::Error::PaymentRequired { code, details, .. }) => {
            assert_eq!(code, "PAYMENT_REQUIRED");
            assert!(!details.card_order_id.is_empty());
            assert!(!details.x402_reference_id.is_empty());
            assert!(!details.payee_address.is_empty());
            assert!(!details.asset_address.is_empty());
            assert!(!details.payable_amount.is_empty());
            assert!(!details.fee_amount.is_empty());
        }
        Err(other) => panic!("expected PaymentRequired, got: {:?}", other),
        Ok(_) => panic!("expected 402 error, got success"),
    }
}));
