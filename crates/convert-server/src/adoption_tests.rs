use super::connection_tests::api;
use super::*;
use chrono::{Duration, Utc};
use convert_domain::{NewNumber, PhoneProvider};
use sqlx::Row;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
#[ignore = "requires disposable VISICONVERT_TEST_DATABASE_URL"]
async fn adoption_is_atomic_retryable_scoped_and_never_backbills() {
    let store = Store::connect(
        &std::env::var("VISICONVERT_TEST_DATABASE_URL").unwrap(),
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let project = Uuid::new_v4();
    let sid = Uuid::new_v4().to_string();
    let phone_value = format!(
        "+1928{:07}",
        1_000_000 + (Uuid::new_v4().as_u128() % 8_000_000)
    );
    let phone = phone_value.as_str();
    let destination = "+16025550100";
    let vault = secrets::Vault::from_hex(&"e".repeat(64)).unwrap();
    // Use the real connection API's encryption purposes.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let writes = Arc::new(AtomicUsize::new(0));
    let inventory = Arc::new(std::sync::Mutex::new(json!([])));
    let calls = Arc::new(std::sync::Mutex::new(json!([])));
    let inv = inventory.clone();
    let records = calls.clone();
    let mutations = writes.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().fallback(move |method:axum::http::Method, uri:axum::http::Uri| {
            let inv=inv.clone(); let records=records.clone();let mutations=mutations.clone();
            async move {
                if method!=axum::http::Method::GET { mutations.fetch_add(1,Ordering::SeqCst);return StatusCode::METHOD_NOT_ALLOWED.into_response(); }
                if uri.path().ends_with("/IncomingPhoneNumbers") { Json(json!({"incoming_phone_numbers":inv.lock().unwrap().clone(),"next_page_uri":null})).into_response() }
                else if uri.path().ends_with("/Calls") { Json(json!({"calls":records.lock().unwrap().clone(),"next_page_uri":null})).into_response() }
                else { StatusCode::NOT_FOUND.into_response() }
            }
        })).await.unwrap();
    });
    let key = "d".repeat(64);
    let app = Arc::new(App {
        store: store.clone(),
        api_key: Uuid::new_v4().to_string(),
        public_url: "https://engine.example.com".into(),
        twilio_account: None,
        twilio_token: None,
        cloud: Some(hosted::Config {
            service_key: key.clone(),
            cloud_origin: origin.clone(),
            app_origin: None,
        }),
        vault: Some(vault),
        twilio_client: convert_adapters::twilio_api::Client::new().unwrap(),
        signalwire_client: convert_adapters::signalwire_api::Client::for_test(origin),
        google: None,
        google_client: convert_adapters::google_api::Client::new().unwrap(),
    });
    // Match the production encryption context, without sending secrets through adoption.
    let token = app
        .vault
        .as_ref()
        .unwrap()
        .seal_signalwire(
            store.workspace,
            "fixture.signalwire.com",
            project,
            "api-token",
            "fixture-token",
        )
        .unwrap();
    let signing = app
        .vault
        .as_ref()
        .unwrap()
        .seal_signalwire(
            store.workspace,
            "fixture.signalwire.com",
            project,
            "signing-key",
            "fixture-signing",
        )
        .unwrap();
    store
        .save_signalwire_connection("fixture.signalwire.com", project, &signing, &token, 0)
        .await
        .unwrap();
    store.record_signalwire_webhook(1).await.unwrap();
    store
        .add_number_with_connection(
            &NewNumber {
                provider: PhoneProvider::Signalwire,
                label: "Carrier test".into(),
                phone_number: phone.into(),
                destination: destination.into(),
                source: "Test".into(),
                campaign: "Test".into(),
            },
            Some(1),
        )
        .await
        .unwrap();
    let number = store.numbers().await.unwrap()[0].id;
    sqlx::query("UPDATE tracking_numbers SET carrier_sid=$2,purchased_at=now()-interval '2 days' WHERE id=$1").bind(number).bind(&sid).execute(&store.pool).await.unwrap();
    sqlx::query("INSERT INTO number_purchases(workspace_id,request_id,phone_number,status,number_id,project_id,managed,created_at) VALUES($1,$2,$3,'registered',$4,$5,false,now()-interval '2 days')")
        .bind(store.workspace).bind(Uuid::new_v4()).bind(phone).bind(number).bind(project).execute(&store.pool).await.unwrap();
    let base = format!(
        "https://engine.example.com/webhooks/signalwire/workspaces/{}",
        store.workspace
    );
    *inventory.lock().unwrap() = json!([{"sid":sid,"account_sid":project,"phone_number":phone,"voice_url":format!("{base}/voice"),"voice_method":"POST","status_callback":format!("{base}/status"),"status_callback_method":"POST","voice_application_sid":null}]);
    let path = format!(
        "/internal/cloud/workspaces/{}/carrier-adoption",
        store.workspace
    );
    let preview = format!("{path}/preview");
    assert_eq!(
        api(app.clone(), "POST", &preview, &app.api_key, json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, plan) = api(app.clone(), "POST", &preview, &key, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let id = Uuid::new_v4();
    let policy = json!({"project_id":project,"revision":1,"state":"active","reason":"paid","valid_until":Utc::now().timestamp()+3600,"release_at":null});
    let input = json!({"adoption_id":id,"plan":plan,"policy":policy});
    let mut wrong = input.clone();
    wrong["plan"]["numbers"][0]["destination"] = json!("+16025550999");
    assert_ne!(
        api(app.clone(), "POST", &path, &key, wrong).await.0,
        StatusCode::OK
    );
    assert!(
        !store
            .signalwire_connection()
            .await
            .unwrap()
            .unwrap()
            .managed
    );
    let mut unpaid = input.clone();
    unpaid["policy"]["state"] = json!("grace");
    assert_ne!(
        api(app.clone(), "POST", &path, &key, unpaid).await.0,
        StatusCode::OK
    );
    inventory.lock().unwrap()[0]["voice_url"] = json!("https://old-route.example");
    assert_ne!(
        api(app.clone(), "POST", &path, &key, input.clone()).await.0,
        StatusCode::OK
    );
    inventory.lock().unwrap()[0]["voice_url"] = json!(format!("{base}/voice"));
    // Shared projects cannot be adopted, including when the other tenant is empty.
    let other = Store {
        pool: store.pool.clone(),
        workspace: Uuid::new_v4(),
    };
    other.migrate().await.unwrap();
    other
        .save_signalwire_connection("fixture.signalwire.com", project, "cipher", "cipher", 0)
        .await
        .unwrap();
    assert_ne!(
        api(app.clone(), "POST", &preview, &key, json!({})).await.0,
        StatusCode::OK
    );
    other.remove_signalwire_connection(1).await.unwrap();
    let before = store.signalwire_connection().await.unwrap().unwrap();
    let (status, receipt) = api(app.clone(), "POST", &path, &key, input.clone()).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert!(store.managed_forwarding_allowed().await.unwrap());
    let after = store.signalwire_connection().await.unwrap().unwrap();
    assert!(after.managed);
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.encrypted_token, before.encrypted_token);
    assert_eq!(after.encrypted_signing_key, before.encrypted_signing_key);
    assert_eq!(after.last_webhook_at, before.last_webhook_at);
    assert_eq!(
        api(app.clone(), "POST", &path, &key, input.clone()).await.1,
        receipt
    );
    let mut retry = input.clone();
    retry["policy"]["valid_until"] = json!(Utc::now().timestamp() + 7200);
    assert_eq!(
        api(app.clone(), "POST", &path, &key, retry).await.1,
        receipt
    );
    assert!(
        other
            .save_signalwire_connection("fixture.signalwire.com", project, "cipher", "cipher", 1)
            .await
            .is_err()
    );
    let row = sqlx::query(
        "SELECT purchased_at,managed_adopted_at,destination FROM tracking_numbers WHERE id=$1",
    )
    .bind(number)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert!(row.get::<chrono::DateTime<Utc>, _>("purchased_at") < Utc::now() - Duration::days(1));
    assert_eq!(row.get::<String, _>("destination"), destination);
    let activation = receipt["activated_at"].as_i64().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let now = Utc::now().timestamp();
    let mut evidence = Vec::new();
    for (start, end, seconds) in [
        (now - 86400, now - 86300, 100),
        (activation - 10, now, 1),
        (activation, now, 0),
    ] {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        evidence.push(json!({"sid":parent,"account_sid":project,"direction":"inbound","to":phone,"status":"completed","start_time":chrono::DateTime::from_timestamp(start,0).unwrap().to_rfc2822(),"end_time":chrono::DateTime::from_timestamp(end,0).unwrap().to_rfc2822(),"duration":seconds}));
        evidence.push(json!({"sid":child,"account_sid":project,"direction":"outbound-dial","parent_call_sid":parent,"status":"completed","end_time":chrono::DateTime::from_timestamp(end,0).unwrap().to_rfc2822(),"duration":seconds}));
    }
    *calls.lock().unwrap() = json!(evidence);
    let (status, report) = api(
        app.clone(),
        "POST",
        &format!("/internal/cloud/workspaces/{}/usage", store.workspace),
        &key,
        json!({"project_id":project,"period_start":now-3*86400,"period_end":now+86400}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["calls"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(report["numbers"][0]["started_at"], activation);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    server.abort();
}
