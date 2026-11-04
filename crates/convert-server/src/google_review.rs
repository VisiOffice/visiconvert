//! Review of the saved Google Ads account's conversion actions for overlap with
//! VisiConvert, and the one change agents may make there: an action's primary/secondary
//! goal setting, and whether an account-level conversion goal (category and origin) is
//! used for bidding. Reading needs `read`; changing either needs `conversions:send`.
//! Nothing here creates, removes, renames or revalues a Google action, and campaign-level
//! goal overrides are never read or changed.
use super::*;
use axum::extract::Query;
use chrono::{Duration, Utc};
use convert_adapters::google_api::{ConversionGoal, ReviewedAction, enum_name};
use convert_store::api_keys::KeyAccess;
use serde::Deserialize;

pub(crate) fn routes() -> Router<Shared> {
    Router::new()
        .route("/conversion-setup/google-review", get(review))
        .route(
            "/conversion-setup/google-actions/{id}/goal",
            axum::routing::put(set_goal),
        )
        .route(
            "/conversion-setup/google-goal",
            axum::routing::put(set_biddable),
        )
}

const UPLOADS: [&str; 2] = ["UPLOAD_CLICKS", "UPLOAD_CALLS"];

fn flag(code: &str, severity: &str, explanation: String, suggestion: &str) -> Value {
    json!({"code":code,"severity":severity,"explanation":explanation,"suggestion":suggestion})
}
/// Google bids toward an action only when it is primary and its account-level goal is
/// biddable. Without a reported goal, primary alone decides.
pub(crate) fn bidding(a: &ReviewedAction) -> bool {
    a.primary && a.goal.as_ref().is_none_or(|g| g.biddable)
}
fn is_call_action(a: &ReviewedAction) -> bool {
    matches!(a.kind.as_str(), "WEBSITE_CALL" | "CLICK_TO_CALL")
        || (a.kind == "GOOGLE_HOSTED"
            && (a.category == "PHONE_CALL_LEAD" || a.name.to_lowercase().contains("call")))
}
/// Plain-language findings per action. `mapped` lists Google action IDs VisiConvert sends to.
pub(crate) fn classify(a: &ReviewedAction, customer: &str, mapped: &[String]) -> Value {
    let is_mapped = mapped.contains(&a.id);
    let enabled = a.status == "ENABLED";
    let bids = bidding(a);
    let mut flags = Vec::new();
    let mut note = Value::Null;
    if is_mapped {
        flags.push(flag(
            "mapped",
            "info",
            "VisiConvert sends conversions to this action.".into(),
            "Keep it. Make sure no other tool uploads to it.",
        ));
    } else if enabled && UPLOADS.contains(&a.kind.as_str()) {
        if a.conversions_90d > 0.0 {
            let last = a
                .last_conversion_date
                .as_deref()
                .map_or(String::new(), |d| format!(", most recently on {d}"));
            if bids {
                flags.push(flag("active_other_uploader", "warning", format!(
                    "Another tool is still uploading conversions to this action ({} in the last 90 days{last}). If it records the same leads VisiConvert sends, Google counts them twice.",
                    a.conversions_90d),
                    "Turn off the other uploader (for example an old call-tracking or CRM integration), or make this action secondary, before turning on automatic sending."));
            } else {
                let why = if a.primary {
                    "Its goal is not used for bidding, so it does not change what Google bids toward"
                } else {
                    "It is secondary, so it is reported but does not count toward your goals or bidding"
                };
                flags.push(flag("active_other_uploader", "info", format!(
                    "Another tool is still uploading conversions to this action ({} in the last 90 days{last}). {why}.",
                    a.conversions_90d),
                    "Turn off the other uploader when it is no longer needed."));
            }
        } else if bids {
            flags.push(flag("dead_upload", "warning",
                "This primary import action has had no conversions in 90 days, so it is probably left over from a previous tracking tool. As a primary action it still counts toward your goals and bidding if anything uploads to it again.".into(),
                "Make it secondary."));
        }
    }
    if !is_mapped && enabled && bids && is_call_action(a) {
        let mut text = "This primary action counts phone calls that VisiConvert also records as leads, so Google would count the same call twice once VisiConvert sends it.".to_string();
        if a.kind == "WEBSITE_CALL" {
            text.push_str(" It also means Google's call tag may replace the phone number on your website with a Google forwarding number, which fights VisiConvert's own number swap (DNI).");
        }
        flags.push(flag("call_overlap", "warning", text,
            if a.kind == "WEBSITE_CALL" { "Make it secondary, and remove Google's call tag or forwarding number from the website." } else { "Make it secondary." }));
    }
    if a.kind == "AD_CALL" {
        note = json!(
            "Calls from ads go straight from the ad to your phone and never reach the website, so VisiConvert does not see them. Keeping this action does not double count."
        );
    }
    if !a.owner.is_empty() && a.owner != customer {
        note = json!(format!(
            "Owned by account {}; its goal can only be changed there.",
            a.owner
        ));
    }
    let mut v = serde_json::to_value(a).unwrap_or_default();
    v["mapped"] = json!(is_mapped);
    v["bidding"] = json!(bids);
    v["flags"] = json!(flags);
    v["note"] = note;
    v
}
fn report(customer: &str, login: &str, entries: &[Value], more: bool) -> Value {
    let warnings: Vec<Value> = entries
        .iter()
        .filter_map(|e| {
            let codes: Vec<&Value> = e["flags"]
                .as_array()?
                .iter()
                .filter(|f| f["severity"] == "warning")
                .map(|f| &f["code"])
                .collect();
            (!codes.is_empty()).then(|| {
                json!({"id":e["id"],"name":e["name"],"type":e["type"],"primary_for_goal":e["primary_for_goal"],"bidding":e["bidding"],"flags":codes})
            })
        })
        .collect();
    let summary = if warnings.is_empty() {
        "No conversion actions in this account look likely to double count with VisiConvert."
            .to_string()
    } else {
        format!(
            "{} conversion action(s) may double count with VisiConvert: {}.",
            warnings.len(),
            warnings
                .iter()
                .map(|w| w["name"].as_str().unwrap_or(""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    json!({"customer_id":customer,"login":login,"actions":entries,"warnings":warnings,"has_more":more,"summary":summary})
}
/// The access account to use: the caller's, the last checked one, or the first that
/// reaches the saved advertising account.
async fn resolve_login(
    a: &App,
    store: &Store,
    token: &str,
    customer: &str,
    given: Option<String>,
) -> Result<String, ApiError> {
    let logins = a.google_client.logins(token).await?;
    if let Some(login) = given {
        return if logins.contains(&login) {
            Ok(login)
        } else {
            Err(convert_adapters::google_api::Error::Access.into())
        };
    }
    let checked = store
        .google_connection()
        .await?
        .validation
        .and_then(|v| v["login_customer_id"].as_str().map(String::from));
    if let Some(login) = checked.filter(|l| logins.contains(l)) {
        return Ok(login);
    }
    for login in logins {
        let (accounts, _) = a.google_client.accounts(token, &login).await?;
        if accounts.iter().any(|x| x.id == customer) {
            return Ok(login);
        }
    }
    Err(ApiError(
        StatusCode::BAD_REQUEST,
        "The saved advertising account is not reachable from the connected Google login.".into(),
    ))
}
struct Snapshot {
    customer: String,
    login: String,
    token: String,
    actions: Vec<ReviewedAction>,
    goals: Vec<ConversionGoal>,
    more: bool,
    mapped: Vec<String>,
}
async fn snapshot(
    a: &App,
    access: &hosted::Access,
    login: Option<String>,
) -> Result<Snapshot, ApiError> {
    let settings = access.store.google_conversion_settings().await?;
    if settings.customer_id.is_empty() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Save the advertising account first (set_google_destination, or Conversions).".into(),
        ));
    }
    let (_, token) = google::token(a, access).await?;
    let login = resolve_login(a, &access.store, &token, &settings.customer_id, login).await?;
    let today = Utc::now().date_naive();
    let day = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
    let review = a
        .google_client
        .review_actions(
            &token,
            &login,
            &settings.customer_id,
            (
                &day(today),
                &day(today - Duration::days(29)),
                &day(today - Duration::days(89)),
            ),
        )
        .await?;
    let mut mapped = access.store.pipeline_action_ids().await?;
    mapped.extend(
        [settings.qualified_action_id, settings.booking_action_id]
            .into_iter()
            .filter(|s| !s.is_empty()),
    );
    Ok(Snapshot {
        customer: settings.customer_id,
        login,
        token,
        actions: review.actions,
        goals: review.goals,
        more: review.more,
        mapped,
    })
}
async fn save(store: &Store, s: &Snapshot) -> Result<Value, ApiError> {
    let entries: Vec<Value> = s
        .actions
        .iter()
        .map(|x| classify(x, &s.customer, &s.mapped))
        .collect();
    let mut value = report(&s.customer, &s.login, &entries, s.more);
    let stored = json!({"customer_id":s.customer,"login":s.login,"warnings":value["warnings"],"summary":value["summary"]});
    store.save_google_review(&s.customer, &stored).await?;
    value["reviewed_at"] = json!(Utc::now());
    Ok(value)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewQuery {
    login: Option<String>,
}
/// read. Reads Google and records the findings for the setup checklist.
async fn review(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    Query(q): Query<ReviewQuery>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    let s = snapshot(&a, &access, q.login).await?;
    Ok(Json(save(&access.store, &s).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Goal {
    primary: bool,
    login: Option<String>,
}
/// The key's actor, or the signed-in user's, for the conversion history.
async fn actor(
    access: &hosted::Access,
    key: &Option<Extension<KeyAccess>>,
) -> Result<String, ApiError> {
    Ok(match key {
        Some(k) => k.actor.clone(),
        None => {
            access
                .store
                .inquiry_actor(access.session_hash.as_deref())
                .await?
        }
    })
}
/// Google hosts these actions itself; the API does not change their primary setting.
fn google_hosted(a: &ReviewedAction) -> bool {
    a.kind == "GOOGLE_HOSTED" || a.origin == "GOOGLE_HOSTED"
}
/// 422 for an action whose primary setting Google will not change, pointing at the
/// account-level goal that controls its bidding when there is one.
fn goal_level_error(a: &ReviewedAction, primary: bool) -> ApiError {
    let Some(g) = &a.goal else {
        return convert_adapters::google_api::Error::NotMutable.into();
    };
    let shared = if a.goal_shared_with.is_empty() {
        "No other conversion action is in that goal.".to_string()
    } else {
        format!(
            "That goal also includes: {}. Changing it affects all of them.",
            a.goal_shared_with.join(", ")
        )
    };
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!(
            "Google does not let the API make \"{}\" ({}, type {}) primary or secondary. Its bidding is controlled by the account-level goal {} / {}, which is {} used for bidding. A goal-level change is available: set_google_goal_biddable (PUT /api/v1/conversion-setup/google-goal) with category {}, origin {} and biddable {}. {shared} It changes what Smart Bidding optimizes for across the account, so name every action in the goal to the owner and get their explicit yes first.",
            a.name,
            a.id,
            a.kind,
            g.category,
            g.origin,
            if g.biddable {
                "currently"
            } else {
                "not currently"
            },
            g.category,
            g.origin,
            primary
        ),
    )
}
/// The changed rows plus the account summary, without every other action.
fn slim(full: &Value, rows: Vec<Value>, key: &str) -> Value {
    let mut out = json!({});
    for field in [
        "customer_id",
        "login",
        "summary",
        "warnings",
        "has_more",
        "reviewed_at",
    ] {
        out[field] = full[field].clone();
    }
    out[key] = if key == "action" {
        rows.into_iter().next().unwrap_or(Value::Null)
    } else {
        json!(rows)
    };
    out
}
/// conversions:send. Toggles primary_for_goal on one action owned by the saved account.
async fn set_goal(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Path(id): Path<String>,
    Json(input): Json<Goal>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use a numeric Google conversion action ID from review_google_conversion_setup".into(),
        ));
    }
    let actor = actor(&access, &key).await?;
    let mut s = snapshot(&a, &access, input.login).await?;
    let customer = s.customer.clone();
    let action = s.actions.iter_mut().find(|x| x.id == id).ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "No such conversion action in the saved advertising account".into(),
    ))?;
    if action.owner != customer {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!(
                "This action is owned by account {}, not the saved advertising account; change it there.",
                action.owner
            ),
        ));
    }
    let before = action.primary;
    if before != input.primary {
        if google_hosted(action) && action.goal.is_some() {
            return Err(goal_level_error(action, input.primary));
        }
        match a
            .google_client
            .set_action_primary(&s.token, &s.login, &customer, &id, input.primary)
            .await
        {
            Err(convert_adapters::google_api::Error::NotMutable) => {
                return Err(goal_level_error(action, input.primary));
            }
            r => r?,
        }
        action.primary = input.primary;
        access
            .store
            .log_google_goal_change(
                &actor,
                &json!({"customer_id":customer,"action_id":id,"name":action.name,"kind":action.kind,"primary_before":before,"primary":input.primary}),
            )
            .await?;
    }
    let full = save(&access.store, &s).await?;
    let row = full["actions"]
        .as_array()
        .and_then(|list| list.iter().find(|x| x["id"] == id.as_str()))
        .cloned()
        .unwrap_or(Value::Null);
    let mut value = slim(&full, vec![row], "action");
    value["changed"] = json!(before != input.primary);
    Ok(Json(value))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Biddable {
    category: String,
    origin: String,
    biddable: bool,
    login: Option<String>,
}
/// conversions:send. Switches one account-level conversion goal of the saved account in or
/// out of bidding. Only `biddable` changes; campaign goal overrides are left alone.
async fn set_biddable(
    State(a): State<Shared>,
    Extension(access): Extension<hosted::Access>,
    key: Option<Extension<KeyAccess>>,
    Json(input): Json<Biddable>,
) -> Api<Value> {
    conversions::owner(&access).await?;
    if !enum_name(&input.category) || !enum_name(&input.origin) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use goal.category and goal.origin from review_google_conversion_setup".into(),
        ));
    }
    let actor = actor(&access, &key).await?;
    let mut s = snapshot(&a, &access, input.login).await?;
    let customer = s.customer.clone();
    let goal = s
        .goals
        .iter_mut()
        .find(|g| g.category == input.category && g.origin == input.origin)
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "No such conversion goal in the saved advertising account".into(),
        ))?;
    let before = goal.biddable;
    let in_goal = |x: &ReviewedAction| x.category == input.category && x.origin == input.origin;
    if before != input.biddable {
        let target = ConversionGoal {
            biddable: input.biddable,
            ..goal.clone()
        };
        a.google_client
            .set_goal_biddable(&s.token, &s.login, &customer, &target)
            .await?;
        *goal = target.clone();
        for x in s.actions.iter_mut().filter(|x| in_goal(x)) {
            x.goal = Some(target.clone());
        }
        let actions: Vec<Value> = s
            .actions
            .iter()
            .filter(|x| in_goal(x))
            .map(|x| json!({"id":x.id,"name":x.name,"type":x.kind,"primary_for_goal":x.primary}))
            .collect();
        access
            .store
            .log_google_goal_biddable_change(
                &actor,
                &json!({"customer_id":customer,"resource_name":target.resource(&customer),"category":target.category,"origin":target.origin,"biddable_before":before,"biddable":target.biddable,"actions":actions}),
            )
            .await?;
    }
    let full = save(&access.store, &s).await?;
    let rows: Vec<Value> = full["actions"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|x| {
                    x["category"] == input.category.as_str() && x["origin"] == input.origin.as_str()
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut value = slim(&full, rows, "actions");
    value["goal"] = json!({"category":input.category,"origin":input.origin,"biddable":input.biddable,"resource_name":format!("customers/{customer}/customerConversionGoals/{}~{}", input.category, input.origin)});
    value["changed"] = json!(before != input.biddable);
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(id: &str, kind: &str, primary: bool, c90: f64) -> ReviewedAction {
        ReviewedAction {
            id: id.into(),
            name: format!("Action {id}"),
            status: "ENABLED".into(),
            kind: kind.into(),
            category: "DEFAULT".into(),
            owner: "1234567890".into(),
            primary,
            conversions_90d: c90,
            ..Default::default()
        }
    }
    fn codes(v: &Value) -> Vec<String> {
        v["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap().to_string())
            .collect()
    }
    #[test]
    fn classification_matches_overlap_rules() {
        let c = "1234567890";
        let mapped = vec!["1".to_string()];
        let check = |a: ReviewedAction, want: &[&str]| {
            assert_eq!(codes(&classify(&a, c, &mapped)), want, "{}", a.kind);
        };
        check(action("1", "UPLOAD_CLICKS", true, 4.0), &["mapped"]);
        check(
            action("2", "UPLOAD_CLICKS", false, 3.0),
            &["active_other_uploader"],
        );
        check(action("3", "UPLOAD_CLICKS", true, 0.0), &["dead_upload"]);
        check(action("4", "UPLOAD_CLICKS", false, 0.0), &[]);
        check(
            action("5", "UPLOAD_CALLS", false, 1.0),
            &["active_other_uploader"],
        );
        check(action("6", "WEBSITE_CALL", true, 9.0), &["call_overlap"]);
        check(action("7", "WEBSITE_CALL", false, 9.0), &[]);
        let mut hosted = action("8", "GOOGLE_HOSTED", true, 2.0);
        hosted.name = "Clicks to call".into();
        check(hosted.clone(), &["call_overlap"]);
        hosted.name = "Directions".into();
        check(hosted, &[]);
        check(action("9", "AD_CALL", true, 5.0), &[]);
        let mut paused = action("10", "UPLOAD_CLICKS", true, 3.0);
        paused.status = "HIDDEN".into();
        check(paused, &[]);
        let website = classify(&action("6", "WEBSITE_CALL", true, 0.0), c, &mapped);
        assert!(
            website["flags"][0]["explanation"]
                .as_str()
                .unwrap()
                .contains("DNI")
        );
        assert!(
            classify(&action("9", "AD_CALL", true, 0.0), c, &mapped)["note"]
                .as_str()
                .unwrap()
                .contains("never reach the website")
        );
        // A secondary uploader is still reported, but only primary ones are warnings.
        let secondary = classify(&action("2", "UPLOAD_CLICKS", false, 3.0), c, &mapped);
        assert_eq!(secondary["flags"][0]["severity"], "info");
        let entries = [
            secondary,
            classify(&action("12", "UPLOAD_CLICKS", true, 3.0), c, &mapped),
            classify(&action("1", "UPLOAD_CLICKS", true, 4.0), c, &mapped),
        ];
        let r = report(c, c, &entries, false);
        assert_eq!(r["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(r["warnings"][0]["id"], "12");
        assert_eq!(r["warnings"][0]["flags"], json!(["active_other_uploader"]));
    }
    /// A primary action whose account-level goal is not biddable does not count for
    /// bidding, so it raises no bidding warnings; without a reported goal, primary decides.
    #[test]
    fn bidding_needs_primary_and_biddable_goal() {
        let c = "1234567890";
        let goal = |biddable| {
            Some(ConversionGoal {
                category: "DEFAULT".into(),
                origin: "WEBSITE".into(),
                biddable,
            })
        };
        let mut a = action("3", "UPLOAD_CLICKS", true, 0.0);
        assert!(bidding(&a));
        a.goal = goal(true);
        assert!(bidding(&a));
        assert_eq!(codes(&classify(&a, c, &[])), ["dead_upload"]);
        a.goal = goal(false);
        assert!(!bidding(&a));
        let v = classify(&a, c, &[]);
        assert_eq!(v["bidding"], false);
        assert!(codes(&v).is_empty(), "{v}");
        let mut hosted = action("8", "GOOGLE_HOSTED", true, 2.0);
        hosted.name = "Clicks to call".into();
        hosted.goal = goal(false);
        assert!(codes(&classify(&hosted, c, &[])).is_empty());
        let mut live = action("2", "UPLOAD_CLICKS", true, 3.0);
        live.goal = goal(false);
        let v = classify(&live, c, &[]);
        assert_eq!(v["flags"][0]["severity"], "info");
        assert!(
            v["flags"][0]["explanation"]
                .as_str()
                .unwrap()
                .contains("not used for bidding")
        );
        let secondary = action("4", "UPLOAD_CLICKS", false, 0.0);
        assert!(!bidding(&secondary));
    }
}
