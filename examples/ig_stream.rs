//! Streams live prices from an **IG** trading account.
//!
//! IG runs Lightstreamer, but reaching it takes one step first: you log in to
//! IG's REST API, and it answers with two tokens and the address of *your*
//! Lightstreamer endpoint. Those tokens become the TLCP credentials.
//!
//! ```text
//! POST /session  ──▶  CST + X-SECURITY-TOKEN headers
//!                     lightstreamerEndpoint + currentAccountId in the body
//!                              │
//!                              ▼
//!            LS_user     = the account id
//!            LS_password = CST-<cst>|XST-<security-token>
//! ```
//!
//! The `CST-…|XST-…` spelling is IG's, not TLCP's: to this crate it is simply
//! a password. Everything after the handshake is ordinary TLCP.
//!
//! # Running it
//!
//! Use a **demo** account. Set your credentials in the environment — never in
//! source — and run:
//!
//! ```text
//! export IG_API_KEY=…          # from My Account ▸ API keys on IG's dashboard
//! export IG_IDENTIFIER=…       # your IG username
//! export IG_PASSWORD=…
//! export IG_ENV=demo           # or `live`; defaults to demo
//! export IG_EPICS=IX.D.FTSE.DAILY.IP,CS.D.EURUSD.MINI.IP   # optional
//!
//! cargo run --example ig_stream
//! ```
//!
//! # Item and field names
//!
//! These belong to IG, not to TLCP, and IG may change them: check the
//! Streaming API Guide at <https://labs.ig.com/streaming-api-guide.html> if a
//! subscription is refused. The shapes used here are:
//!
//! | Item | Mode | What it carries | Used here |
//! |---|---|---|---|
//! | `MARKET:<epic>` | MERGE | live bid/offer and market state for one instrument | yes |
//! | `ACCOUNT:<accountId>` | MERGE | running P&L, margin and available funds | yes |
//! | `TRADE:<accountId>` | DISTINCT | deal confirmations and position updates, each its own event | no — add it the same way, in `SubscriptionMode::Distinct` |
//! | `CHART:<epic>:<scale>` | MERGE | OHLC candles as they form | no |
//!
//! A refused subscription arrives as `Rejected` with IG's own code, which is
//! usually enough to tell a wrong field name from a wrong epic.

use std::env;
use std::process::ExitCode;

use futures_util::StreamExt;
use lightstreamer_rs::{
    Client, ClientConfig, Credentials, FieldSchema, ItemGroup, ServerAddress, Subscription,
    SubscriptionEvent, SubscriptionMode,
};
use serde::Deserialize;

/// IG's demo gateway.
const DEMO_GATEWAY: &str = "https://demo-api.ig.com/gateway/deal";

/// IG's live gateway. Real money; this example never places an order, but be
/// deliberate about pointing anything at it.
const LIVE_GATEWAY: &str = "https://api.ig.com/gateway/deal";

/// Instruments to watch when `IG_EPICS` is not set.
const DEFAULT_EPICS: &str = "IX.D.FTSE.DAILY.IP";

/// The price fields asked for on a `MARKET:` item.
const MARKET_FIELDS: [&str; 5] = [
    "BID",
    "OFFER",
    "UPDATE_TIME",
    "MARKET_STATE",
    "MARKET_DELAY",
];

/// The account fields asked for on an `ACCOUNT:` item.
const ACCOUNT_FIELDS: [&str; 4] = ["PNL", "AVAILABLE_CASH", "MARGIN", "EQUITY"];

/// What IG's `POST /session` (Version 2) returns in its body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IgSession {
    /// The address of the Lightstreamer server assigned to this account.
    lightstreamer_endpoint: String,
    /// The account the session is bound to; this is the TLCP user.
    current_account_id: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = required("IG_API_KEY")?;
    let identifier = required("IG_IDENTIFIER")?;
    let password = required("IG_PASSWORD")?;
    let gateway = match env::var("IG_ENV").as_deref() {
        Ok("live") => LIVE_GATEWAY,
        _ => DEMO_GATEWAY,
    };

    // --- Step 1: log in to IG's REST API ---------------------------------
    println!("logging in to {gateway} …");
    let http = reqwest::Client::new();
    let response = http
        .post(format!("{gateway}/session"))
        .header("X-IG-API-KEY", &api_key)
        // Version 2 is the one that returns `lightstreamerEndpoint`.
        .header("Version", "2")
        .json(&serde_json::json!({
            "identifier": identifier,
            "password": password,
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        // IG puts a machine-readable `errorCode` in the body; it is far more
        // useful than the status line. It never contains the password.
        let body = response.text().await.unwrap_or_default();
        return Err(format!("IG refused the login ({status}): {body}").into());
    }

    // The tokens live in the headers, the endpoint in the body.
    let cst = header(&response, "CST")?;
    let security_token = header(&response, "X-SECURITY-TOKEN")?;
    let session: IgSession = response.json().await?;

    println!("logged in as {}", session.current_account_id);
    println!("streaming endpoint: {}", session.lightstreamer_endpoint);

    // --- Step 2: connect to Lightstreamer with those tokens ---------------
    // IG's spelling, not TLCP's. To this crate it is just a password, and it
    // is never logged or echoed in an error.
    let ls_password = format!("CST-{cst}|XST-{security_token}");

    let config = ClientConfig::builder(ServerAddress::try_new(&session.lightstreamer_endpoint)?)
        .with_credentials(Credentials::new(&session.current_account_id, ls_password))
        .build()?;

    let (client, _session_events) = Client::connect(config).await?;
    println!("connected to Lightstreamer\n");

    // --- Step 3: subscribe ------------------------------------------------
    let epics = env::var("IG_EPICS").unwrap_or_else(|_| DEFAULT_EPICS.to_owned());
    let market_items: Vec<String> = epics
        .split(',')
        .map(str::trim)
        .filter(|epic| !epic.is_empty())
        .map(|epic| format!("MARKET:{epic}"))
        .collect();

    println!("watching {} market(s): {epics}", market_items.len());

    let markets = client
        .subscribe(
            Subscription::new(
                SubscriptionMode::Merge,
                ItemGroup::from_items(market_items)?,
                FieldSchema::from_fields(MARKET_FIELDS)?,
            )
            // IG delivers a snapshot of the current price on subscription.
            .with_snapshot(lightstreamer_rs::Snapshot::On),
        )
        .await?;

    let account = client
        .subscribe(Subscription::new(
            SubscriptionMode::Merge,
            ItemGroup::from_items([format!("ACCOUNT:{}", session.current_account_id)])?,
            FieldSchema::from_fields(ACCOUNT_FIELDS)?,
        ))
        .await?;

    // Merge both streams so one loop can print whatever arrives first.
    let mut events = futures_util::stream::select(
        markets.map(|event| ("market", event)),
        account.map(|event| ("account", event)),
    );

    while let Some((source, event)) = events.next().await {
        match event {
            SubscriptionEvent::Activated {
                item_count,
                field_count,
                ..
            } => println!("[{source}] subscribed: {item_count} items × {field_count} fields"),

            SubscriptionEvent::Update(update) => {
                let values: Vec<String> = update
                    .changed_fields()
                    .map(|field| format!("{}={}", field.name(), field.value().text_or("(null)")))
                    .collect();
                println!(
                    "[{source}] {:<28} {}",
                    update.item_name(),
                    values.join("  ")
                );
            }

            SubscriptionEvent::Rejected(error) => {
                // Usually a wrong epic or a field name IG does not publish.
                eprintln!("[{source}] IG refused the subscription: {error}");
            }

            SubscriptionEvent::Unsubscribed => println!("[{source}] unsubscribed"),

            other => println!("[{source}] {other:?}"),
        }
    }

    client.disconnect().await?;
    Ok(())
}

/// Reads a required environment variable, with a message that says how to get it.
fn required(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| {
        format!("{name} is not set — see the header of this example for what it needs")
    })
}

/// Pulls a header IG returns, failing with a useful message if it is missing.
fn header(response: &reqwest::Response, name: &str) -> Result<String, String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| format!("IG's login response carried no {name} header"))
}
