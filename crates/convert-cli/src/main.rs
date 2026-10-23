mod client;
mod config;
mod mcp;
mod ops;
mod reference;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use client::Client;
use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, IsTerminal, Write};

#[derive(Parser)]
#[command(
    name = "visiconvert",
    version,
    about = "Manage VisiConvert call tracking from the command line, or run it as an MCP server for AI agents"
)]
struct Cli {
    /// Print JSON instead of tables
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Save the server URL and workspace key for later commands
    Login {
        /// Server URL, e.g. https://convert.example.com (https unless it is localhost)
        #[arg(long)]
        url: String,
        /// Read the key from stdin instead of prompting
        #[arg(long)]
        key_stdin: bool,
    },
    /// Forget the saved server URL and key
    Logout,
    /// Server health, Twilio connection and what to do next
    Status,
    /// Connect a phone provider (workspace owners only)
    #[command(subcommand)]
    Connect(Connect),
    /// Tracking numbers
    #[command(subcommand)]
    Numbers(Numbers),
    /// Leads, qualification and bookings
    #[command(subcommand)]
    Leads(Leads),
    /// Reports
    #[command(subcommand)]
    Report(Report),
    /// Run as an MCP server on stdio for AI agents
    Mcp {
        /// Expose tools that add numbers and leads, qualify leads and record bookings
        #[arg(long)]
        allow_changes: bool,
    },
    /// Print a Markdown reference page for the docs site
    #[command(hide = true)]
    Reference { page: Page },
}

#[derive(Clone, ValueEnum)]
enum Page {
    Cli,
    Mcp,
}

#[derive(Subcommand)]
enum Connect {
    /// Save Twilio credentials. The auth token is read from stdin, never from arguments.
    Twilio {
        /// Twilio Account SID, starting with AC
        #[arg(long)]
        account_sid: String,
        /// Read the auth token from stdin instead of prompting
        #[arg(long)]
        token_stdin: bool,
    },
}

#[derive(Subcommand)]
enum Numbers {
    /// Tracking numbers in this workspace
    List,
    /// Numbers in your Twilio account that are not tracked yet
    Available,
    /// Track a Twilio number for one source and campaign
    Add {
        /// The Twilio number, e.g. +15125550142
        phone_number: String,
        /// The business line calls ring through to
        #[arg(long)]
        forward_to: String,
        /// Marketing source, e.g. "Google Ads" or "Yard signs"
        #[arg(long)]
        source: String,
        /// Campaign within the source
        #[arg(long)]
        campaign: String,
        /// Defaults to the campaign
        #[arg(long)]
        label: Option<String>,
    },
}

#[derive(Subcommand)]
enum Leads {
    /// Leads, newest first
    List(ListArgs),
    /// Add a lead that did not arrive by phone
    Add {
        /// Who the lead is
        #[arg(long)]
        name: String,
        /// Phone or email
        #[arg(long)]
        contact: String,
        /// Marketing source
        #[arg(long)]
        source: String,
        /// Campaign within the source
        #[arg(long, default_value = "")]
        campaign: String,
        /// Free-text notes
        #[arg(long, default_value = "")]
        notes: String,
        /// Your ID for this lead; repeating it will not create a duplicate
        #[arg(long, default_value = "")]
        external_id: String,
    },
    /// Mark a lead qualified, unqualified, spam or unreviewed
    Qualify {
        /// Lead ID or a unique prefix of at least 6 characters
        lead: String,
        /// qualified, unqualified, spam or unreviewed
        qualification: String,
        /// Replace the lead's notes
        #[arg(long)]
        notes: Option<String>,
    },
    /// Record a booked job, or cancel one
    Outcome {
        /// Lead ID or a unique prefix of at least 6 characters
        lead: String,
        /// Job or invoice number; reusing it revises that booking
        #[arg(long = "ref")]
        reference: String,
        /// Booked value in USD, e.g. 1250.00
        #[arg(long, conflicts_with = "canceled")]
        booked: Option<String>,
        /// Cancel the booking with this reference
        #[arg(long)]
        canceled: bool,
    },
    /// Bookings recorded on a lead
    Outcomes {
        /// Lead ID or a unique prefix of at least 6 characters
        lead: String,
    },
}

#[derive(Args)]
struct ListArgs {
    /// 7d, 24h, 30m, or a date like 2026-10-01
    #[arg(long)]
    since: Option<String>,
    /// Only leads from this source
    #[arg(long)]
    source: Option<String>,
    /// unreviewed, qualified, unqualified or spam
    #[arg(long)]
    status: Option<String>,
    /// Most leads to show
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// Include sample leads
    #[arg(long)]
    demo: bool,
}

#[derive(Subcommand)]
enum Report {
    /// Leads, qualified leads, bookings and revenue by source
    Sources,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    if let Err(error) = run(cli).await {
        if json {
            println!("{}", serde_json::json!({"error": error.to_string()}));
        } else {
            eprintln!("error: {error:#}");
        }
        std::process::exit(1);
    }
}

fn connect() -> Result<Client> {
    let config = config::load()?;
    Client::new(&config.url, &config.api_key)
}

async fn run(cli: Cli) -> Result<()> {
    let json = cli.json;
    match cli.command {
        Command::Login { url, key_stdin } => {
            let url = url.trim_end_matches('/').to_owned();
            if !url.starts_with("https://")
                && !url.starts_with("http://localhost")
                && !url.starts_with("http://127.0.0.1")
            {
                bail!("Use an https:// URL. Plain http is only accepted for localhost.");
            }
            let key = secret("Workspace key", key_stdin)?;
            let client = Client::new(&url, &key)?;
            let workspace = client.bootstrap().await?;
            let path = config::save(&config::Config { url, api_key: key })?;
            let name = workspace["name"].as_str().unwrap_or("workspace").to_owned();
            output(
                json,
                &serde_json::json!({"logged_in": name, "config": path}),
                || format!("Logged in to {name}. Saved to {}", path.display()),
            )
        }
        Command::Logout => {
            let removed = config::remove()?;
            output(json, &serde_json::json!({"removed": removed}), || {
                if removed {
                    "Logged out.".into()
                } else {
                    "Nothing saved.".into()
                }
            })
        }
        Command::Status => {
            let status = ops::status(&connect()?).await?;
            output(json, &status, || {
                let t = &status.twilio;
                let mut lines = vec![
                    format!(
                        "Server     {} ({})",
                        status.server,
                        if status.healthy {
                            "healthy"
                        } else {
                            "not responding to /health"
                        }
                    ),
                    format!(
                        "Workspace  {}",
                        status.workspace["name"].as_str().unwrap_or("?")
                    ),
                ];
                if t["visible"] == false {
                    lines.push("Twilio     only the workspace owner can see the connection".into());
                } else {
                    let account = t["account_sid"].as_str().unwrap_or("not connected");
                    lines.push(format!(
                        "Twilio     {account} ({})",
                        t["mode"].as_str().unwrap_or("?")
                    ));
                    if let Some(url) = t["voice_url"].as_str() {
                        lines.push(format!("Voice URL  {url}"));
                    }
                    if let Some(url) = t["status_url"].as_str() {
                        lines.push(format!("Status URL {url}"));
                    }
                }
                lines.push(format!("Numbers    {} tracking", status.tracking_numbers));
                lines.extend(status.next_steps.iter().map(|s| format!("Next       {s}")));
                lines.join("\n")
            })
        }
        Command::Connect(Connect::Twilio {
            account_sid,
            token_stdin,
        }) => {
            let token = secret("Twilio auth token", token_stdin)?;
            let connection = ops::connect_twilio(&connect()?, account_sid.trim(), &token).await?;
            output(json, &connection, || {
                format!(
                    "Twilio connected ({}).\nVoice URL  {}\nStatus URL {}",
                    connection["account_sid"].as_str().unwrap_or("?"),
                    connection["voice_url"].as_str().unwrap_or("?"),
                    connection["status_url"].as_str().unwrap_or("?")
                )
            })
        }
        Command::Numbers(Numbers::List) => {
            let numbers: Vec<_> = connect()?
                .numbers()
                .await?
                .into_iter()
                .filter(|n| !n.is_demo)
                .collect();
            output(json, &numbers, || {
                table(
                    &["NUMBER", "FORWARDS TO", "SOURCE", "CAMPAIGN", "LABEL"],
                    numbers
                        .iter()
                        .map(|n| {
                            vec![
                                n.phone_number.clone(),
                                n.destination.clone(),
                                n.source.clone(),
                                n.campaign.clone(),
                                n.label.clone(),
                            ]
                        })
                        .collect(),
                    "No tracking numbers yet. Run `visiconvert numbers available`.",
                )
            })
        }
        Command::Numbers(Numbers::Available) => {
            let available = ops::available_numbers(&connect()?).await?;
            output(json, &available, || {
                let rows = available["untracked"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .map(|n| {
                                vec![
                                    n["phone_number"].as_str().unwrap_or("").into(),
                                    n["friendly_name"].as_str().unwrap_or("").into(),
                                ]
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut text = table(
                    &["NUMBER", "TWILIO NAME"],
                    rows,
                    "Every number in your Twilio account is already tracked.",
                );
                if available["has_more"] == true {
                    text.push_str(
                        "\nTwilio has more numbers than one page; only the first page is shown.",
                    );
                }
                text
            })
        }
        Command::Numbers(Numbers::Add {
            phone_number,
            forward_to,
            source,
            campaign,
            label,
        }) => {
            let added = ops::add_number(
                &connect()?,
                ops::AddNumber {
                    phone_number,
                    forward_to,
                    source,
                    campaign,
                    label,
                },
            )
            .await?;
            output(json, &added, || {
                let mut text = format!(
                    "Tracking {} for {} · {}, forwarding to {}.\n\nOne step left: {}",
                    added["tracking_number"].as_str().unwrap_or(""),
                    added["source"].as_str().unwrap_or(""),
                    added["campaign"].as_str().unwrap_or(""),
                    added["forwards_to"].as_str().unwrap_or(""),
                    added["manual_step"].as_str().unwrap_or("")
                );
                for (name, key) in [("Voice URL ", "voice_url"), ("Status URL", "status_url")] {
                    if let Some(url) = added[key].as_str().filter(|u| !u.is_empty()) {
                        text.push_str(&format!("\n  {name} {url}"));
                    }
                }
                text
            })
        }
        Command::Leads(Leads::List(args)) => {
            let filter = ops::LeadFilter {
                since: args
                    .since
                    .map(|s| {
                        ops::parse_since(
                            &s,
                            chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()),
                        )
                    })
                    .transpose()?,
                source: args.source,
                qualification: args
                    .status
                    .map(|s| ops::parse_qualification(&s).map(|q| q.as_str().to_owned()))
                    .transpose()?,
                include_demo: args.demo,
                limit: Some(args.limit),
            };
            let leads = ops::list_leads(&connect()?, &filter).await?;
            output(json, &leads, || {
                table(
                    &[
                        "ID", "WHEN", "NAME", "SOURCE", "CAMPAIGN", "STATUS", "CALL", "BOOKED",
                    ],
                    leads
                        .iter()
                        .map(|l| {
                            vec![
                                l.id.to_string()[..8].to_owned(),
                                l.created_at.format("%Y-%m-%d %H:%M").to_string(),
                                l.name.clone(),
                                l.source.clone(),
                                l.campaign.clone(),
                                l.qualification.clone(),
                                match (&l.call_status, l.duration_seconds) {
                                    (Some(s), Some(d)) => format!("{s} {}:{:02}", d / 60, d % 60),
                                    (Some(s), None) => s.clone(),
                                    _ => l.channel.clone(),
                                },
                                if l.booked_minor > 0 {
                                    ops::money(l.booked_minor)
                                } else {
                                    String::new()
                                },
                            ]
                        })
                        .collect(),
                    "No leads match.",
                )
            })
        }
        Command::Leads(Leads::Add {
            name,
            contact,
            source,
            campaign,
            notes,
            external_id,
        }) => {
            let id = ops::add_lead(
                &connect()?,
                convert_domain::NewLead {
                    external_id,
                    name,
                    contact,
                    source,
                    campaign,
                    notes,
                },
            )
            .await?;
            output(json, &serde_json::json!({"id": id}), || {
                format!("Added lead {id}")
            })
        }
        Command::Leads(Leads::Qualify {
            lead,
            qualification,
            notes,
        }) => {
            let result = ops::qualify(
                &connect()?,
                &lead,
                ops::parse_qualification(&qualification)?,
                notes,
            )
            .await?;
            output(json, &result, || {
                format!(
                    "{} is now {}",
                    result["name"].as_str().unwrap_or("Lead"),
                    result["qualification"].as_str().unwrap_or("")
                )
            })
        }
        Command::Leads(Leads::Outcome {
            lead,
            reference,
            booked,
            canceled,
        }) => {
            let status = if canceled {
                ops::OutcomeStatus::Canceled
            } else {
                ops::OutcomeStatus::Booked
            };
            if !canceled && booked.is_none() {
                bail!("Give --booked AMOUNT, or --canceled");
            }
            let amount = booked.map(|b| ops::parse_money(&b)).transpose()?;
            let result =
                ops::record_outcome(&connect()?, &lead, &reference, amount, status).await?;
            output(json, &result, || {
                format!(
                    "{} {} {} for {} (revision {})",
                    result["reference"].as_str().unwrap_or(""),
                    result["status"].as_str().unwrap_or(""),
                    result["amount"].as_str().unwrap_or(""),
                    result["name"].as_str().unwrap_or(""),
                    result["revision"]
                )
            })
        }
        Command::Leads(Leads::Outcomes { lead }) => {
            let result = ops::outcomes(&connect()?, &lead).await?;
            output(json, &result, || {
                let rows = result["outcomes"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .map(|o| {
                                ["reference", "status", "amount", "revision"]
                                    .iter()
                                    .map(|k| plain(&o[*k]))
                                    .collect()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                table(
                    &["REF", "STATUS", "AMOUNT", "REVISION"],
                    rows,
                    "No bookings on this lead.",
                )
            })
        }
        Command::Report(Report::Sources) => {
            let report = ops::source_report(&connect()?).await?;
            output(json, &report, || {
                let rows = report["sources"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .map(|s| {
                                ["source", "leads", "qualified", "bookings", "booked"]
                                    .iter()
                                    .map(|k| plain(&s[*k]))
                                    .collect()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                table(
                    &["SOURCE", "LEADS", "QUALIFIED", "BOOKINGS", "BOOKED"],
                    rows,
                    "No leads yet.",
                )
            })
        }
        Command::Mcp { allow_changes } => {
            let client = config::load()
                .and_then(|c| Client::new(&c.url, &c.api_key))
                .map_err(|e| e.to_string());
            mcp::Server::new(client, allow_changes).run().await
        }
        Command::Reference { page } => {
            print!("{}", reference_page(page));
            Ok(())
        }
    }
}

fn reference_page(page: Page) -> String {
    match page {
        Page::Cli => reference::cli(&Cli::command()),
        Page::Mcp => mcp::reference(),
    }
}

fn output<T: Serialize>(json: bool, value: &T, human: impl FnOnce() -> String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", human());
    }
    Ok(())
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn table(headers: &[&str], rows: Vec<Vec<String>>, empty: &str) -> String {
    if rows.is_empty() {
        return empty.to_owned();
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count().min(40));
        }
    }
    let line = |cells: Vec<String>| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let c: String = c.chars().take(40).collect();
                format!("{c:<width$}", width = widths[i])
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    let mut out = vec![line(headers.iter().map(|h| h.to_string()).collect())];
    out.extend(rows.into_iter().map(line));
    out.join("\n")
}

/// Reads a secret from stdin. On a terminal, echo is turned off while typing.
fn secret(label: &str, from_stdin: bool) -> Result<String> {
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal() && !from_stdin;
    if interactive {
        eprint!("{label}: ");
        std::io::stderr().flush()?;
        echo(false);
    }
    let mut value = String::new();
    let read = stdin.lock().read_line(&mut value);
    if interactive {
        echo(true);
        eprintln!();
    }
    read.context("Could not read from stdin")?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{label} is empty");
    }
    Ok(value)
}

fn echo(on: bool) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("stty")
            .arg(if on { "echo" } else { "-echo" })
            .stdin(std::process::Stdio::inherit())
            .status();
    }
    #[cfg(not(unix))]
    let _ = on;
}
