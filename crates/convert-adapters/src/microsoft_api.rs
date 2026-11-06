//! Microsoft identity platform OAuth and bounded Microsoft Advertising (Bing Ads API v13)
//! SOAP transport: account discovery, offline conversion goals, and offline conversion
//! uploads and adjustments. Requests are built from whitelisted JSON; upstream bodies,
//! messages and tokens are never returned to callers or logged.
//!
//! Shapes follow the v13 reference (SOAP templates and element order):
//! - <https://learn.microsoft.com/en-us/advertising/campaign-management-service/applyofflineconversions?view=bingads-13>
//! - <https://learn.microsoft.com/en-us/advertising/campaign-management-service/applyofflineconversionadjustments?view=bingads-13>
//! - <https://learn.microsoft.com/en-us/advertising/campaign-management-service/getconversiongoalsbyids?view=bingads-13>
//! - <https://learn.microsoft.com/en-us/advertising/campaign-management-service/addconversiongoals?view=bingads-13>
//! - <https://learn.microsoft.com/en-us/advertising/customer-management-service/getuser?view=bingads-13>
//! - <https://learn.microsoft.com/en-us/advertising/customer-management-service/searchaccounts?view=bingads-13>
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

/// Requested at consent and on every token request (scope is required on refresh).
pub const SCOPE: &str = "https://ads.microsoft.com/msads.manage offline_access";
const MANAGE: &str = "https://ads.microsoft.com/msads.manage";
pub const AUTHORIZE: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const CAMPAIGN_NS: &str = "https://bingads.microsoft.com/CampaignManagement/v13";
const CUSTOMER_NS: &str = "https://bingads.microsoft.com/Customer/v13";
const ENTITIES_NS: &str = "https://bingads.microsoft.com/Customer/v13/Entities";
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// Offline conversion goal categories (OfflineConversionGoal GoalCategory).
pub const GOAL_CATEGORIES: &[&str] = &[
    "AddToCart",
    "BeginCheckout",
    "BookAppointment",
    "Contact",
    "GetDirections",
    "Other",
    "OutboundClick",
    "PageView",
    "Purchase",
    "RequestQuote",
    "Signup",
    "SubmitLeadForm",
    "Subscribe",
];
/// Microsoft's maximum conversion window, in minutes (90 days).
pub const MAX_WINDOW_MINUTES: i64 = 129_600;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error(
        "Microsoft authorization expired or was revoked, or the developer token was rejected. Reconnect Microsoft Advertising, or ask the administrator to check the developer token."
    )]
    Credentials,
    #[error(
        "Microsoft did not grant Microsoft Advertising access. Reconnect and allow VisiConvert to manage your Microsoft Advertising accounts."
    )]
    Scope,
    #[error(
        "Microsoft rejected the OAuth application configuration. Ask the administrator to check its client ID, secret and redirect URI."
    )]
    Configuration,
    #[error(
        "Microsoft Advertising denied access. Check that the signed-in user can manage this ad account."
    )]
    Access,
    #[error("Microsoft Advertising is unavailable or rate limited. Try again shortly.")]
    Unavailable,
    #[error(
        "An offline conversion goal with this name already exists in the Microsoft Advertising account."
    )]
    GoalNameExists,
    #[error(
        "Microsoft goal creation could not be confirmed. Retry to recover a goal an earlier attempt created; do not create a replacement manually."
    )]
    CreationUncertain,
    #[error(
        "Microsoft Advertising returned an unexpected response. Try again or ask the administrator to check the API configuration."
    )]
    Response,
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    token_type: String,
}
/// A role the signed-in user holds for one customer (manager account).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Role {
    pub customer_id: String,
    pub account_ids: Vec<String>,
    pub linked_account_ids: Vec<String>,
}
/// An ad account, with the customer whose ID goes in the CustomerId header for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub number: String,
    pub customer_id: String,
    pub parent_customer_id: String,
    pub status: String,
    pub currency: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Goal {
    pub id: String,
    pub name: String,
    pub status: String,
    pub count_type: String,
    pub window_minutes: i64,
    pub scope: String,
    pub category: String,
    pub revenue_type: String,
    /// ConversionGoalType, for example OfflineConversion or Event.
    pub kind: String,
    pub exclude_from_bidding: bool,
    /// Active offline conversion goals can receive uploads.
    pub eligible: bool,
}
/// What VisiConvert creates for one pipeline action.
#[derive(Debug, Clone)]
pub struct GoalSpec {
    pub name: String,
    pub category: &'static str,
    pub count_type: &'static str,
}
impl GoalSpec {
    /// Unique counts one conversion per click (a lead); All counts each (an order).
    pub fn pipeline(name: &str, counting: &str, category: Option<&str>) -> Option<Self> {
        let once = match counting {
            "once_per_click" => true,
            "every" => false,
            _ => return None,
        };
        let category = match category {
            Some(c) => *GOAL_CATEGORIES.iter().find(|v| **v == c)?,
            None if once => "SubmitLeadForm",
            None => "Purchase",
        };
        goal_name(name).then(|| Self {
            name: name.into(),
            category,
            count_type: if once { "Unique" } else { "All" },
        })
    }
}
/// Microsoft account, customer and goal IDs are longs.
pub fn numeric_id(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}
/// Goal names are at most 100 characters and unique per customer.
pub fn goal_name(s: &str) -> bool {
    let n = s.chars().count();
    (1..=100).contains(&n) && !s.chars().any(char::is_control) && s.trim() == s
}
fn token_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16384 && !s.chars().any(char::is_control)
}
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Owned, namespace-free view of a parsed response element.
#[derive(Debug, Default, Clone)]
struct El {
    name: String,
    text: String,
    nil: bool,
    /// The xsi:type attribute, without its prefix.
    attrs_type: Option<String>,
    children: Vec<El>,
}
impl El {
    fn parse(xml: &str) -> Option<Self> {
        // roxmltree rejects DTDs by default, so no entity expansion is possible.
        let doc = roxmltree::Document::parse(xml).ok()?;
        fn walk(n: roxmltree::Node) -> El {
            El {
                name: n.tag_name().name().to_string(),
                text: n
                    .children()
                    .filter(|c| c.is_text())
                    .filter_map(|c| c.text())
                    .collect::<String>()
                    .trim()
                    .to_string(),
                nil: n
                    .attributes()
                    .any(|a| a.name() == "nil" && a.value() == "true"),
                attrs_type: n
                    .attributes()
                    .find(|a| a.name() == "type")
                    .map(|a| a.value().rsplit(':').next().unwrap_or_default().to_string()),
                children: n.children().filter(|c| c.is_element()).map(walk).collect(),
            }
        }
        Some(walk(doc.root_element()))
    }
    fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    fn text_of(&self, name: &str) -> String {
        self.child(name)
            .filter(|c| !c.nil)
            .map(|c| c.text.clone())
            .unwrap_or_default()
    }
    fn descendants(&self, name: &str, out: &mut Vec<El>) {
        for c in &self.children {
            if c.name == name {
                out.push(c.clone());
            }
            c.descendants(name, out);
        }
    }
}
/// One provider error: numeric Code and symbolic ErrorCode, never its free-text message.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderError {
    pub code: i64,
    pub error_code: String,
}
fn errors(root: &El, kinds: &[&str]) -> Vec<ProviderError> {
    let mut out = Vec::new();
    for kind in kinds {
        let mut found = Vec::new();
        root.descendants(kind, &mut found);
        for e in found {
            let error_code = e.text_of("ErrorCode");
            out.push(ProviderError {
                code: e.text_of("Code").parse().unwrap_or(-1),
                // Symbolic constants only; anything else is dropped.
                error_code: if error_code.len() <= 80
                    && error_code.bytes().all(|b| b.is_ascii_alphanumeric())
                {
                    error_code
                } else {
                    String::new()
                },
            });
        }
    }
    out
}
/// A parsed SOAP reply: the operation's response element, or a fault's errors.
enum Reply {
    Ok { body: El, tracking: String },
    Fault(Vec<ProviderError>),
}
fn reply(xml: &str) -> Option<Reply> {
    let root = El::parse(xml)?;
    if root.name != "Envelope" {
        return None;
    }
    let body = root.child("Body")?;
    if let Some(fault) = body.child("Fault") {
        return Some(Reply::Fault(errors(
            fault,
            &["AdApiError", "OperationError", "BatchError"],
        )));
    }
    let tracking = root
        .child("Header")
        .map(|h| h.text_of("TrackingId"))
        .unwrap_or_default();
    Some(Reply::Ok {
        body: body.children.first()?.clone(),
        tracking: if tracking.len() <= 100 && !tracking.chars().any(char::is_control) {
            tracking
        } else {
            String::new()
        },
    })
}
fn setup_error(errors: &[ProviderError]) -> Error {
    if errors.iter().any(|e| {
        matches!(e.code, 105 | 109)
            || matches!(
                e.error_code.as_str(),
                "InvalidCredentials" | "AuthenticationTokenExpired"
            )
    }) {
        Error::Credentials
    } else if errors
        .iter()
        .any(|e| e.code == 117 || e.code == 0 || e.error_code == "CallRateExceeded")
    {
        Error::Unavailable
    } else {
        Error::Access
    }
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    oauth: String,
    customer: String,
    campaign: String,
}
impl Client {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .user_agent("VisiConvert/0.1")
                .build()
                .map_err(|_| Error::Unavailable)?,
            oauth: "https://login.microsoftonline.com/common/oauth2/v2.0".into(),
            customer: "https://clientcenter.api.bingads.microsoft.com/Api/CustomerManagement/v13/CustomerManagementService.svc".into(),
            campaign: "https://campaign.api.bingads.microsoft.com/Api/Advertiser/CampaignManagement/v13/CampaignManagementService.svc".into(),
        })
    }
    #[cfg(feature = "test-support")]
    pub fn for_test(origin: String) -> Self {
        let mut c = Self::new().unwrap();
        c.oauth = format!("{origin}/oauth2/v2.0");
        c.customer = format!("{origin}/customer");
        c.campaign = format!("{origin}/campaign");
        c
    }
    async fn body(response: &mut reqwest::Response) -> Option<String> {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len() + chunk.len() > 4_194_304 {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).ok()
    }
    async fn token(&self, form: &[(&str, &str)], initial: bool) -> Result<Tokens, Error> {
        let mut response = self
            .http
            .post(format!("{}/token", self.oauth))
            .form(form)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        let text = Self::body(&mut response).await.ok_or(Error::Response)?;
        let value: Value = serde_json::from_str(&text).map_err(|_| {
            if status >= 500 {
                Error::Unavailable
            } else {
                Error::Response
            }
        })?;
        match status {
            200 => (),
            429 | 500..=599 => return Err(Error::Unavailable),
            400 | 401 => {
                return Err(match value["error"].as_str() {
                    Some("invalid_grant" | "interaction_required") => Error::Credentials,
                    Some("invalid_scope" | "consent_required") => Error::Scope,
                    Some("invalid_client" | "unauthorized_client" | "invalid_request") => {
                        Error::Configuration
                    }
                    _ => Error::Response,
                });
            }
            _ => return Err(Error::Response),
        }
        let token: Tokens = serde_json::from_value(value).map_err(|_| Error::Response)?;
        if !token_text(&token.access_token) || !token.token_type.eq_ignore_ascii_case("bearer") {
            return Err(Error::Response);
        }
        if initial && !token.refresh_token.as_deref().is_some_and(token_text) {
            return Err(Error::Credentials);
        }
        if token
            .refresh_token
            .as_deref()
            .is_some_and(|s| !token_text(s))
        {
            return Err(Error::Response);
        }
        if token
            .scope
            .as_deref()
            .is_some_and(|s| !s.split_whitespace().any(|v| v == MANAGE))
            || (initial && token.scope.is_none())
        {
            return Err(Error::Scope);
        }
        Ok(token)
    }
    pub async fn exchange(
        &self,
        id: &str,
        secret: &str,
        redirect: &str,
        code: &str,
        verifier: &str,
    ) -> Result<Tokens, Error> {
        self.token(
            &[
                ("client_id", id),
                ("client_secret", secret),
                ("scope", SCOPE),
                ("redirect_uri", redirect),
                ("code", code),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
            ],
            true,
        )
        .await
    }
    /// Microsoft may rotate the refresh token on every refresh; callers persist any new one.
    pub async fn refresh(&self, id: &str, secret: &str, refresh: &str) -> Result<Tokens, Error> {
        self.token(
            &[
                ("client_id", id),
                ("client_secret", secret),
                ("scope", SCOPE),
                ("refresh_token", refresh),
                ("grant_type", "refresh_token"),
            ],
            false,
        )
        .await
    }
    fn envelope(ns: &str, headers: &[(&str, &str)], body: &str) -> String {
        let headers: String = headers
            .iter()
            .map(|(k, v)| format!("<h:{k} xmlns:h=\"{ns}\">{}</h:{k}>", escape(v)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:i=\"{XSI}\"><s:Header>{headers}</s:Header><s:Body>{body}</s:Body></s:Envelope>"
        )
    }
    /// Read-only and setup calls: transport problems are retryable, faults are mapped.
    async fn call(&self, url: &str, action: &str, envelope: String) -> Result<(El, String), Error> {
        let mut response = self
            .http
            .post(url)
            .header("content-type", "text/xml; charset=utf-8")
            .header("soapaction", action)
            .body(envelope)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        let text = Self::body(&mut response).await;
        match text.as_deref().and_then(reply) {
            Some(Reply::Ok { body, tracking }) if status == 200 => Ok((body, tracking)),
            Some(Reply::Fault(errors)) => Err(setup_error(&errors)),
            _ if status == 429 || status >= 500 => Err(Error::Unavailable),
            _ if status == 401 => Err(Error::Credentials),
            _ => Err(Error::Response),
        }
    }
    fn customer_headers<'a>(token: &'a str, developer: &'a str) -> [(&'a str, &'a str); 2] {
        [
            ("AuthenticationToken", token),
            ("DeveloperToken", developer),
        ]
    }
    fn campaign_headers<'a>(
        token: &'a str,
        developer: &'a str,
        customer: &'a str,
        account: &'a str,
    ) -> [(&'a str, &'a str); 4] {
        [
            ("AuthenticationToken", token),
            ("CustomerAccountId", account),
            ("CustomerId", customer),
            ("DeveloperToken", developer),
        ]
    }
    /// The signed-in user's ID and customer roles (GetUser with no UserId).
    pub async fn user(&self, token: &str, developer: &str) -> Result<(String, Vec<Role>), Error> {
        let body = format!(
            "<GetUserRequest xmlns=\"{CUSTOMER_NS}\"><UserId i:nil=\"true\"/></GetUserRequest>"
        );
        let (r, _) = self
            .call(
                &self.customer,
                "GetUser",
                Self::envelope(
                    CUSTOMER_NS,
                    &Self::customer_headers(token, developer),
                    &body,
                ),
            )
            .await?;
        parse_user(&r).ok_or(Error::Response)
    }
    /// Ad accounts matching one predicate, ordered by name, at most 1000.
    async fn search(
        &self,
        token: &str,
        developer: &str,
        field: &str,
        value: &str,
    ) -> Result<(Vec<Account>, bool), Error> {
        if !numeric_id(value) {
            return Err(Error::Access);
        }
        let body = format!(
            "<SearchAccountsRequest xmlns=\"{CUSTOMER_NS}\"><Predicates xmlns:e=\"{ENTITIES_NS}\"><e:Predicate><e:Field>{field}</e:Field><e:Operator>Equals</e:Operator><e:Value>{value}</e:Value></e:Predicate></Predicates><Ordering xmlns:e=\"{ENTITIES_NS}\"><e:OrderBy><e:Field>Name</e:Field><e:Order>Ascending</e:Order></e:OrderBy></Ordering><PageInfo xmlns:e=\"{ENTITIES_NS}\"><e:Index>0</e:Index><e:Size>1000</e:Size></PageInfo></SearchAccountsRequest>"
        );
        let (r, _) = self
            .call(
                &self.customer,
                "SearchAccounts",
                Self::envelope(
                    CUSTOMER_NS,
                    &Self::customer_headers(token, developer),
                    &body,
                ),
            )
            .await?;
        let accounts = parse_accounts(&r).ok_or(Error::Response)?;
        let more = accounts.len() >= 1000;
        Ok((accounts, more))
    }
    /// Every ad account the signed-in user can reach, each with the customer to operate
    /// from: the owning customer when the user has a role there, otherwise the customer
    /// whose role links the account (an agency managing a client account).
    pub async fn accounts(
        &self,
        token: &str,
        developer: &str,
    ) -> Result<(Vec<Account>, bool), Error> {
        let (user, roles) = self.user(token, developer).await?;
        let (mut accounts, more) = self.search(token, developer, "UserId", &user).await?;
        for a in &mut accounts {
            a.customer_id = operating_customer(&roles, &a.id, &a.parent_customer_id);
        }
        Ok((accounts, more))
    }
    /// One reachable ad account by ID, or None.
    pub async fn account(
        &self,
        token: &str,
        developer: &str,
        id: &str,
    ) -> Result<Option<Account>, Error> {
        let (_, roles) = self.user(token, developer).await?;
        let (accounts, _) = self.search(token, developer, "AccountId", id).await?;
        Ok(accounts.into_iter().find(|a| a.id == id).map(|mut a| {
            a.customer_id = operating_customer(&roles, &a.id, &a.parent_customer_id);
            a
        }))
    }
    /// Every conversion goal of the given types visible in the account (null IDs request
    /// them all): offline goals only, or every web and offline type for a review.
    pub async fn goals(
        &self,
        token: &str,
        developer: &str,
        customer: &str,
        account: &str,
        all_types: bool,
    ) -> Result<Vec<Goal>, Error> {
        if !numeric_id(customer) || !numeric_id(account) {
            return Err(Error::Access);
        }
        let types = if all_types {
            "Url Duration PagesViewedPerVisit Event AppInstall OfflineConversion"
        } else {
            "OfflineConversion"
        };
        let body = format!(
            "<GetConversionGoalsByIdsRequest xmlns=\"{CAMPAIGN_NS}\"><ConversionGoalIds i:nil=\"true\"/><ConversionGoalTypes>{types}</ConversionGoalTypes><ReturnAdditionalFields>GoalCategory</ReturnAdditionalFields></GetConversionGoalsByIdsRequest>"
        );
        let (r, _) = self
            .call(
                &self.campaign,
                "GetConversionGoalsByIds",
                Self::envelope(
                    CAMPAIGN_NS,
                    &Self::campaign_headers(token, developer, customer, account),
                    &body,
                ),
            )
            .await?;
        parse_goals(&r).ok_or(Error::Response)
    }
    /// Add one account-scoped offline conversion goal with variable revenue (values come
    /// from each upload) and a 90-day window. Returns its ID.
    pub async fn add_goal(
        &self,
        token: &str,
        developer: &str,
        customer: &str,
        account: &str,
        spec: &GoalSpec,
    ) -> Result<String, Error> {
        if !numeric_id(customer) || !numeric_id(account) || !goal_name(&spec.name) {
            return Err(Error::Access);
        }
        let body = add_goal_body(spec);
        let envelope = Self::envelope(
            CAMPAIGN_NS,
            &Self::campaign_headers(token, developer, customer, account),
            &body,
        );
        let result = self
            .http
            .post(&self.campaign)
            .header("content-type", "text/xml; charset=utf-8")
            .header("soapaction", "AddConversionGoals")
            .body(envelope)
            .send()
            .await;
        let mut response = match result {
            Ok(r) => r,
            Err(e) if e.is_connect() => return Err(Error::Unavailable),
            // The request may have reached Microsoft; a retry recovers it by name.
            Err(_) => return Err(Error::CreationUncertain),
        };
        let status = response.status().as_u16();
        let text = Self::body(&mut response).await;
        match text.as_deref().and_then(reply) {
            Some(Reply::Ok { body, .. }) if status == 200 => parse_added_goal(&body),
            Some(Reply::Fault(errors)) => Err(
                if errors.iter().any(|e| {
                    matches!(
                        e.error_code.as_str(),
                        "ConversionGoalNameAlreadyExists" | "DuplicateConversionGoalName"
                    )
                }) {
                    Error::GoalNameExists
                } else {
                    setup_error(&errors)
                },
            ),
            _ => Err(Error::CreationUncertain),
        }
    }
}
fn add_goal_body(spec: &GoalSpec) -> String {
    // Element order follows the ConversionGoal WSDL sequence; omitted elements are optional.
    format!(
        "<AddConversionGoalsRequest xmlns=\"{CAMPAIGN_NS}\"><ConversionGoals><ConversionGoal i:type=\"OfflineConversionGoal\"><ConversionWindowInMinutes>{MAX_WINDOW_MINUTES}</ConversionWindowInMinutes><CountType>{}</CountType><GoalCategory>{}</GoalCategory><Name>{}</Name><Revenue><Type>VariableValue</Type></Revenue><Scope>Account</Scope><Status>Active</Status></ConversionGoal></ConversionGoals></AddConversionGoalsRequest>",
        spec.count_type,
        spec.category,
        escape(&spec.name)
    )
}
fn parse_added_goal(body: &El) -> Result<String, Error> {
    let errors = errors(body, &["BatchError"]);
    if errors.iter().any(|e| {
        matches!(
            e.error_code.as_str(),
            "ConversionGoalNameAlreadyExists" | "DuplicateConversionGoalName"
        )
    }) {
        return Err(Error::GoalNameExists);
    }
    if !errors.is_empty() {
        return Err(setup_error(&errors));
    }
    let id = body
        .child("ConversionGoalIds")
        .and_then(|ids| ids.children.first())
        .filter(|v| !v.nil)
        .map(|v| v.text.clone())
        .filter(|v| numeric_id(v))
        .ok_or(Error::Response)?;
    Ok(id)
}
fn longs(el: Option<&El>) -> Vec<String> {
    el.map(|e| {
        e.children
            .iter()
            .filter(|c| !c.nil && numeric_id(&c.text))
            .map(|c| c.text.clone())
            .collect()
    })
    .unwrap_or_default()
}
fn parse_user(r: &El) -> Option<(String, Vec<Role>)> {
    let id = r.child("User")?.text_of("Id");
    if !numeric_id(&id) {
        return None;
    }
    let roles = r
        .child("CustomerRoles")
        .map(|roles| {
            roles
                .all("CustomerRole")
                .map(|role| Role {
                    customer_id: role.text_of("CustomerId"),
                    account_ids: longs(role.child("AccountIds")),
                    linked_account_ids: longs(role.child("LinkedAccountIds")),
                })
                .filter(|role| numeric_id(&role.customer_id))
                .collect()
        })
        .unwrap_or_default();
    Some((id, roles))
}
fn parse_accounts(r: &El) -> Option<Vec<Account>> {
    let Some(list) = r.child("Accounts").filter(|a| !a.nil) else {
        return Some(vec![]);
    };
    list.all("AdvertiserAccount")
        .map(|a| {
            let account = Account {
                id: a.text_of("Id"),
                name: a.text_of("Name").chars().take(300).collect(),
                number: a.text_of("Number").chars().take(40).collect(),
                customer_id: String::new(),
                parent_customer_id: a.text_of("ParentCustomerId"),
                status: a
                    .text_of("AccountLifeCycleStatus")
                    .chars()
                    .take(40)
                    .collect(),
                currency: a.text_of("CurrencyCode").chars().take(10).collect(),
            };
            (numeric_id(&account.id) && numeric_id(&account.parent_customer_id)).then_some(account)
        })
        .collect()
}
/// CustomerId to send for an account: its parent customer when the user has a role
/// there, else the customer whose role links the account, else the parent.
pub fn operating_customer(roles: &[Role], account: &str, parent: &str) -> String {
    if roles.iter().any(|r| r.customer_id == parent) {
        return parent.into();
    }
    roles
        .iter()
        .find(|r| {
            r.linked_account_ids.iter().any(|a| a == account)
                || r.account_ids.iter().any(|a| a == account)
        })
        .map_or_else(|| parent.to_string(), |r| r.customer_id.clone())
}
fn parse_goals(r: &El) -> Option<Vec<Goal>> {
    let Some(list) = r.child("ConversionGoals").filter(|g| !g.nil) else {
        return Some(vec![]);
    };
    let goals: Vec<Goal> = list
        .all("ConversionGoal")
        .filter(|g| !g.nil)
        .filter_map(|g| {
            let id = g.text_of("Id");
            let name = g.text_of("Name");
            let kind = g.text_of("Type");
            if !numeric_id(&id) || name.is_empty() || name.chars().count() > 100 {
                return None;
            }
            let status = g.text_of("Status");
            let offline = kind == "OfflineConversion"
                || (kind.is_empty() && g.attrs_type.as_deref() == Some("OfflineConversionGoal"));
            Some(Goal {
                kind: if kind.is_empty() && offline {
                    "OfflineConversion".into()
                } else {
                    kind.chars().take(40).collect()
                },
                exclude_from_bidding: g.text_of("ExcludeFromBidding") == "true",
                eligible: offline && status == "Active",
                id,
                name,
                window_minutes: g.text_of("ConversionWindowInMinutes").parse().unwrap_or(0),
                count_type: g.text_of("CountType").chars().take(20).collect(),
                scope: g.text_of("Scope").chars().take(20).collect(),
                category: g.text_of("GoalCategory").chars().take(40).collect(),
                revenue_type: g
                    .child("Revenue")
                    .map(|r| r.text_of("Type"))
                    .unwrap_or_default()
                    .chars()
                    .take(20)
                    .collect(),
                status: status.chars().take(20).collect(),
            })
        })
        .collect();
    Some(goals)
}

/// Sanitized delivery outcomes. Upstream messages may contain identifiers; only the
/// symbolic error code is kept.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryError {
    /// Not applied and safe to try again later (throttled, internal error, connect failure).
    Retry,
    /// Microsoft does not accept uploads for this goal yet (about 2 hours after creation).
    GoalNotReady,
    /// The request may have been applied. Never replayed automatically.
    Uncertain,
    Authorization,
    /// Explicitly rejected; carries Microsoft's symbolic error code when one was given.
    Rejected(String),
}
impl DeliveryError {
    pub fn message(&self) -> String {
        match self {
            Self::Retry => "Microsoft Advertising is rate limited or unavailable; retry scheduled.".into(),
            Self::GoalNotReady => "Microsoft is not accepting conversions for this goal yet. New offline goals accept uploads about 2 hours after creation; retry scheduled.".into(),
            Self::Uncertain => "Microsoft Advertising may have received this request. It is not sent again automatically; check the conversion in Microsoft Advertising.".into(),
            Self::Authorization => "Microsoft rejected the authorization, developer token or account access. Reconnect Microsoft Advertising or check the ad account.".into(),
            Self::Rejected(code) if code.is_empty() => "Microsoft Advertising rejected this conversion. Check the goal name, click ID and conversion window.".into(),
            Self::Rejected(code) => format!("Microsoft Advertising rejected this conversion ({code}). Check the goal name, click ID and conversion window."),
        }
    }
}
fn delivery_error(errors: &[ProviderError]) -> DeliveryError {
    let has = |codes: &[i64], names: &[&str]| {
        errors
            .iter()
            .any(|e| codes.contains(&e.code) || names.contains(&e.error_code.as_str()))
    };
    if has(&[5615], &["OfflineConversionNotAcceptedForGoal"]) {
        DeliveryError::GoalNotReady
    } else if has(
        &[105, 106, 109],
        &[
            "InvalidCredentials",
            "UserIsNotAuthorized",
            "AuthenticationTokenExpired",
            "InvalidAccount",
        ],
    ) {
        DeliveryError::Authorization
    } else if has(
        &[0, 117, 5612],
        &[
            "InternalError",
            "CallRateExceeded",
            "OfflineConversionsApplyFailed",
        ],
    ) {
        DeliveryError::Retry
    } else {
        DeliveryError::Rejected(
            errors
                .iter()
                .map(|e| e.error_code.as_str())
                .find(|c| !c.is_empty())
                .unwrap_or("")
                .into(),
        )
    }
}
/// `yyyy-mm-ddThh:mm:ss[.fff]Z`, UTC only.
fn utc_time(s: &str) -> bool {
    let b = s.as_bytes();
    (20..=24).contains(&b.len())
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[b.len() - 1] == b'Z'
        && s.chars().all(|c| c.is_ascii_digit() || "-T:.Z".contains(c))
}
fn money(v: &Value) -> Result<Option<String>, DeliveryError> {
    if v.is_null() {
        return Ok(None);
    }
    let cents = v
        .as_i64()
        .filter(|c| (0..=9_000_000_000_000).contains(c))
        .ok_or(DeliveryError::Rejected(String::new()))?;
    Ok(Some(format!("{}.{:02}", cents / 100, cents % 100)))
}
/// Validated SOAP operation and body for a stored delivery request. Requests are JSON
/// whitelists: an upload, or a Restate/Retract adjustment of an earlier upload.
pub fn request_body(r: &Value) -> Result<(&'static str, String), DeliveryError> {
    let bad = || DeliveryError::Rejected(String::new());
    let text = |k: &str| r[k].as_str().unwrap_or("");
    let (name, click, time) = (text("goal_name"), text("click"), text("conversion_time"));
    if !numeric_id(text("customer_id"))
        || !numeric_id(text("account_id"))
        || !goal_name(name)
        || click.is_empty()
        || click.len() > 200
        || click.chars().any(|c| c.is_control() || c.is_whitespace())
        || !utc_time(time)
        || text("currency") != "USD"
    {
        return Err(bad());
    }
    let value = money(&r["value_minor"])?;
    match text("kind") {
        "upload" => {
            let value = value
                .map(|v| format!("<ConversionCurrencyCode>USD</ConversionCurrencyCode><ConversionName>{}</ConversionName><ConversionTime>{time}</ConversionTime><ConversionValue>{v}</ConversionValue>", escape(name)))
                .unwrap_or_else(|| format!("<ConversionName>{}</ConversionName><ConversionTime>{time}</ConversionTime>", escape(name)));
            Ok((
                "ApplyOfflineConversions",
                format!(
                    "<ApplyOfflineConversionsRequest xmlns=\"{CAMPAIGN_NS}\"><OfflineConversions><OfflineConversion>{value}<MicrosoftClickId>{}</MicrosoftClickId></OfflineConversion></OfflineConversions></ApplyOfflineConversionsRequest>",
                    escape(click)
                ),
            ))
        }
        kind @ ("restate" | "retract") => {
            let at = text("adjustment_time");
            if !utc_time(at) {
                return Err(bad());
            }
            let head = if kind == "restate" {
                let v = value.ok_or_else(bad)?;
                format!(
                    "<AdjustmentCurrencyCode>USD</AdjustmentCurrencyCode><AdjustmentTime>{at}</AdjustmentTime><AdjustmentType>Restate</AdjustmentType><AdjustmentValue>{v}</AdjustmentValue>"
                )
            } else {
                format!(
                    "<AdjustmentTime>{at}</AdjustmentTime><AdjustmentType>Retract</AdjustmentType>"
                )
            };
            Ok((
                "ApplyOfflineConversionAdjustments",
                format!(
                    "<ApplyOfflineConversionAdjustmentsRequest xmlns=\"{CAMPAIGN_NS}\"><OfflineConversionAdjustments><OfflineConversionAdjustment>{head}<ConversionName>{}</ConversionName><ConversionTime>{time}</ConversionTime><MicrosoftClickId>{}</MicrosoftClickId></OfflineConversionAdjustment></OfflineConversionAdjustments></ApplyOfflineConversionAdjustmentsRequest>",
                    escape(name),
                    escape(click)
                ),
            ))
        }
        _ => Err(bad()),
    }
}
impl Client {
    /// Send one upload or adjustment. Ok carries Microsoft's TrackingId. Any response
    /// that does not prove the request was refused is Uncertain, never retried.
    pub async fn apply(
        &self,
        token: &str,
        developer: &str,
        request: &Value,
    ) -> Result<Value, DeliveryError> {
        let (action, body) = request_body(request)?;
        let customer = request["customer_id"].as_str().unwrap_or("");
        let account = request["account_id"].as_str().unwrap_or("");
        let envelope = Self::envelope(
            CAMPAIGN_NS,
            &Self::campaign_headers(token, developer, customer, account),
            &body,
        );
        let mut response = self
            .http
            .post(&self.campaign)
            .header("content-type", "text/xml; charset=utf-8")
            .header("soapaction", action)
            .body(envelope)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    DeliveryError::Retry
                } else {
                    DeliveryError::Uncertain
                }
            })?;
        let status = response.status().as_u16();
        let text = Self::body(&mut response).await;
        match text.as_deref().and_then(reply) {
            Some(Reply::Ok { body, tracking }) if status == 200 => {
                let partial = errors(&body, &["BatchError"]);
                if partial.is_empty() {
                    Ok(json!({"tracking_id":tracking}))
                } else {
                    Err(delivery_error(&partial))
                }
            }
            // A SOAP fault is Microsoft explicitly refusing the whole request.
            Some(Reply::Fault(errors)) if !errors.is_empty() => Err(delivery_error(&errors)),
            _ if status == 429 => Err(DeliveryError::Retry),
            _ => Err(DeliveryError::Uncertain),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn upload() -> Value {
        json!({"kind":"upload","customer_id":"111","account_id":"222","goal_name":"VisiConvert - order <A&B>","click":"abc123","conversion_time":"2026-11-04T17:02:00.000Z","value_minor":41250,"currency":"USD"})
    }
    #[test]
    fn upload_body_is_escaped_ordered_and_whitelisted() {
        let (action, body) = request_body(&upload()).unwrap();
        assert_eq!(action, "ApplyOfflineConversions");
        assert!(body.contains("<ConversionCurrencyCode>USD</ConversionCurrencyCode><ConversionName>VisiConvert - order &lt;A&amp;B&gt;</ConversionName><ConversionTime>2026-11-04T17:02:00.000Z</ConversionTime><ConversionValue>412.50</ConversionValue><MicrosoftClickId>abc123</MicrosoftClickId>"), "{body}");
        let mut none = upload();
        none["value_minor"] = Value::Null;
        let (_, body) = request_body(&none).unwrap();
        assert!(!body.contains("ConversionValue") && !body.contains("CurrencyCode"));
        for (k, v) in [
            ("click", json!("")),
            ("click", json!("a b")),
            ("goal_name", json!("")),
            ("goal_name", json!("x".repeat(101))),
            ("conversion_time", json!("2026-11-04 17:02:00")),
            ("conversion_time", json!("2026-11-04T17:02:00+01:00")),
            ("customer_id", json!("abc")),
            ("currency", json!("EUR")),
            ("value_minor", json!(-1)),
            ("kind", json!("delete")),
        ] {
            let mut r = upload();
            r[k] = v;
            assert!(request_body(&r).is_err(), "{k}");
        }
    }
    #[test]
    fn adjustments_follow_the_documented_element_order() {
        let mut r = upload();
        r["kind"] = json!("restate");
        assert!(request_body(&r).is_err(), "adjustment_time is required");
        r["adjustment_time"] = json!("2026-11-05T10:00:00.000Z");
        let (action, body) = request_body(&r).unwrap();
        assert_eq!(action, "ApplyOfflineConversionAdjustments");
        assert!(body.contains("<AdjustmentCurrencyCode>USD</AdjustmentCurrencyCode><AdjustmentTime>2026-11-05T10:00:00.000Z</AdjustmentTime><AdjustmentType>Restate</AdjustmentType><AdjustmentValue>412.50</AdjustmentValue><ConversionName>"), "{body}");
        r["value_minor"] = Value::Null;
        assert!(request_body(&r).is_err(), "restate needs a value");
        r["kind"] = json!("retract");
        let (_, body) = request_body(&r).unwrap();
        assert!(body.contains("<AdjustmentTime>2026-11-05T10:00:00.000Z</AdjustmentTime><AdjustmentType>Retract</AdjustmentType><ConversionName>"));
        assert!(!body.contains("AdjustmentValue") && !body.contains("AdjustmentCurrencyCode"));
    }
    #[test]
    fn replies_faults_and_partial_errors_are_parsed_without_messages() {
        let ok = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Header><TrackingId xmlns="https://bingads.microsoft.com/CampaignManagement/v13">trk-1</TrackingId></s:Header><s:Body><ApplyOfflineConversionsResponse xmlns="https://bingads.microsoft.com/CampaignManagement/v13"><PartialErrors i:nil="true" xmlns:i="http://www.w3.org/2001/XMLSchema-instance"/></ApplyOfflineConversionsResponse></s:Body></s:Envelope>"#;
        let Some(Reply::Ok { body, tracking }) = reply(ok) else {
            panic!()
        };
        assert_eq!(tracking, "trk-1");
        assert!(errors(&body, &["BatchError"]).is_empty());
        let partial = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><ApplyOfflineConversionsResponse xmlns="https://bingads.microsoft.com/CampaignManagement/v13"><PartialErrors><BatchError><Code>5615</Code><ErrorCode>OfflineConversionNotAcceptedForGoal</ErrorCode><Index>0</Index><Message>PRIVATE click abc123</Message></BatchError></PartialErrors></ApplyOfflineConversionsResponse></s:Body></s:Envelope>"#;
        let Some(Reply::Ok { body, .. }) = reply(partial) else {
            panic!()
        };
        let e = errors(&body, &["BatchError"]);
        assert_eq!(delivery_error(&e), DeliveryError::GoalNotReady);
        assert!(!format!("{e:?}").contains("PRIVATE"));
        let fault = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Server</faultcode><faultstring>PRIVATE</faultstring><detail><AdApiFaultDetail xmlns="https://adapi.microsoft.com"><Errors><AdApiError><Code>105</Code><ErrorCode>InvalidCredentials</ErrorCode><Message>PRIVATE</Message></AdApiError></Errors></AdApiFaultDetail></detail></s:Fault></s:Body></s:Envelope>"#;
        let Some(Reply::Fault(e)) = reply(fault) else {
            panic!()
        };
        assert_eq!(delivery_error(&e), DeliveryError::Authorization);
        assert_eq!(setup_error(&e), Error::Credentials);
        for (code, name, expected) in [
            (117, "CallRateExceeded", DeliveryError::Retry),
            (0, "InternalError", DeliveryError::Retry),
            (5612, "OfflineConversionsApplyFailed", DeliveryError::Retry),
            (
                5601,
                "OfflineConversionMicrosoftClickIdInvalid",
                DeliveryError::Rejected("OfflineConversionMicrosoftClickIdInvalid".into()),
            ),
        ] {
            let e = [ProviderError {
                code,
                error_code: name.into(),
            }];
            assert_eq!(delivery_error(&e), expected, "{name}");
        }
        assert!(reply("<!DOCTYPE x [<!ENTITY a 'b'>]><x>&a;</x>").is_none());
        assert!(reply("not xml").is_none());
    }
    #[test]
    fn users_accounts_and_goals_parse() {
        let user = r#"<GetUserResponse xmlns="https://bingads.microsoft.com/Customer/v13"><User xmlns:e="https://bingads.microsoft.com/Customer/v13/Entities"><e:ContactInfo><e:Id>9</e:Id></e:ContactInfo><e:CustomerId>500</e:CustomerId><e:Id>42</e:Id></User><CustomerRoles xmlns:e="https://bingads.microsoft.com/Customer/v13/Entities"><e:CustomerRole><e:RoleId>41</e:RoleId><e:CustomerId>500</e:CustomerId><e:AccountIds xmlns:a="http://schemas.microsoft.com/2003/10/Serialization/Arrays"><a:long>7</a:long></e:AccountIds><e:LinkedAccountIds xmlns:a="http://schemas.microsoft.com/2003/10/Serialization/Arrays"><a:long>8</a:long></e:LinkedAccountIds></e:CustomerRole></CustomerRoles></GetUserResponse>"#;
        let (id, roles) = parse_user(&El::parse(user).unwrap()).unwrap();
        assert_eq!(id, "42");
        assert_eq!(roles[0].linked_account_ids, vec!["8"]);
        assert_eq!(operating_customer(&roles, "7", "500"), "500");
        assert_eq!(operating_customer(&roles, "8", "900"), "500");
        assert_eq!(operating_customer(&roles, "3", "900"), "900");
        let accounts = r#"<SearchAccountsResponse xmlns="https://bingads.microsoft.com/Customer/v13"><Accounts xmlns:e="https://bingads.microsoft.com/Customer/v13/Entities"><e:AdvertiserAccount><e:CurrencyCode>USD</e:CurrencyCode><e:Id>7</e:Id><e:Name>Quarry &amp; Co</e:Name><e:Number>F1</e:Number><e:ParentCustomerId>500</e:ParentCustomerId><e:AccountLifeCycleStatus>Active</e:AccountLifeCycleStatus><e:BusinessAddress><e:Id>99</e:Id></e:BusinessAddress></e:AdvertiserAccount></Accounts></SearchAccountsResponse>"#;
        let a = parse_accounts(&El::parse(accounts).unwrap()).unwrap();
        assert_eq!(a[0].id, "7");
        assert_eq!(a[0].name, "Quarry & Co");
        assert_eq!(a[0].status, "Active");
        let goals = r#"<GetConversionGoalsByIdsResponse xmlns="https://bingads.microsoft.com/CampaignManagement/v13" xmlns:i="http://www.w3.org/2001/XMLSchema-instance"><ConversionGoals><ConversionGoal i:type="OfflineConversionGoal"><ConversionWindowInMinutes>43200</ConversionWindowInMinutes><CountType>Unique</CountType><GoalCategory>SubmitLeadForm</GoalCategory><Id>11</Id><Name>Lead</Name><Revenue><Type>NoValue</Type></Revenue><Scope>Account</Scope><Status>Active</Status><Type>OfflineConversion</Type></ConversionGoal><ConversionGoal i:nil="true"/><ConversionGoal i:type="OfflineConversionGoal"><Id>12</Id><Name>Old</Name><Status>Paused</Status><Type>OfflineConversion</Type></ConversionGoal></ConversionGoals></GetConversionGoalsByIdsResponse>"#;
        let g = parse_goals(&El::parse(goals).unwrap()).unwrap();
        assert_eq!(g.len(), 2);
        assert!(g[0].eligible && !g[1].eligible);
        assert_eq!(g[0].window_minutes, 43200);
        assert_eq!(g[0].count_type, "Unique");
    }
    #[test]
    fn goal_creation_body_and_specs() {
        let spec = GoalSpec::pipeline("VisiConvert - quote", "once_per_click", None).unwrap();
        assert_eq!(
            (spec.count_type, spec.category),
            ("Unique", "SubmitLeadForm")
        );
        let every = GoalSpec::pipeline("x", "every", Some("RequestQuote")).unwrap();
        assert_eq!((every.count_type, every.category), ("All", "RequestQuote"));
        assert!(GoalSpec::pipeline("x", "every", Some("Lead")).is_none());
        let body = add_goal_body(&spec);
        assert!(body.contains("<ConversionGoal i:type=\"OfflineConversionGoal\"><ConversionWindowInMinutes>129600</ConversionWindowInMinutes><CountType>Unique</CountType><GoalCategory>SubmitLeadForm</GoalCategory><Name>VisiConvert - quote</Name><Revenue><Type>VariableValue</Type></Revenue><Scope>Account</Scope><Status>Active</Status></ConversionGoal>"));
    }
}
