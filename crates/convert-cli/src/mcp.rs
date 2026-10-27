//! A stdio MCP server over the same operations as the CLI. Read tools are always on; tools that
//! change data appear only when started with --allow-changes. Twilio credentials are never
//! accepted here, so they stay out of an agent's context.
use crate::{client::Client, ops};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub struct Server {
    client: Result<Client, String>,
    allow_changes: bool,
}

struct Tool {
    name: &'static str,
    description: &'static str,
    changes: bool,
    idempotent: bool,
    schema: fn() -> Value,
}

fn none() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

fn lead_id() -> Value {
    json!({"type": "string", "description": "Lead ID, or a unique prefix of at least 6 characters"})
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "get_status",
        description: "Server health, workspace, Twilio and SignalWire connections (including provider webhook URLs) and suggested next steps. Call this first.",
        changes: false,
        idempotent: true,
        schema: none,
    },
    Tool {
        name: "list_numbers",
        description: "Tracking numbers in this workspace with their source, campaign and forwarding destination.",
        changes: false,
        idempotent: true,
        schema: none,
    },
    Tool {
        name: "list_signalwire_numbers",
        description: "Existing US voice numbers in the connected SignalWire project that are not tracked yet. Connect credentials in the workspace UI first.",
        changes: false,
        idempotent: true,
        schema: none,
    },
    Tool {
        name: "list_twilio_numbers",
        description: "Numbers in the connected Twilio account that are not tracked yet, so they can be assigned to a campaign.",
        changes: false,
        idempotent: true,
        schema: none,
    },
    Tool {
        name: "list_leads",
        description: "Leads (calls and other inquiries), newest first.",
        changes: false,
        idempotent: true,
        schema: || {
            json!({"type": "object", "additionalProperties": false, "properties": {
                "since": {"type": "string", "description": "7d, 24h, 30m, or a date like 2026-10-01"},
                "source": {"type": "string"},
                "qualification": {"type": "string", "enum": ["unreviewed", "qualified", "unqualified", "spam"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50},
                "include_demo": {"type": "boolean", "default": false}
            }})
        },
    },
    Tool {
        name: "get_lead_outcomes",
        description: "Bookings and cancellations recorded on a lead.",
        changes: false,
        idempotent: true,
        schema: || json!({"type": "object", "additionalProperties": false, "required": ["lead_id"], "properties": {"lead_id": lead_id()}}),
    },
    Tool {
        name: "source_report",
        description: "Leads, qualified leads, bookings and booked revenue by marketing source.",
        changes: false,
        idempotent: true,
        schema: none,
    },
    Tool {
        name: "add_tracking_number",
        description: "Track a number already in the selected Twilio or SignalWire account for one source and campaign, forwarding calls to the business line. Afterwards the number's webhooks must be set in the provider console; the result says how.",
        changes: true,
        idempotent: false,
        schema: || {
            json!({"type": "object", "additionalProperties": false, "required": ["phone_number", "forward_to", "source", "campaign"], "properties": {
                "provider": {"type": "string", "enum": ["twilio", "signalwire"], "default": "twilio"},
            "phone_number": {"type": "string", "description": "The existing provider number to track, e.g. +15125550142"},
                "forward_to": {"type": "string", "description": "The business line calls ring through to"},
                "source": {"type": "string", "description": "Marketing source, e.g. Google Ads or Yard signs"},
                "campaign": {"type": "string"},
                "label": {"type": "string", "description": "Defaults to the campaign"}
            }})
        },
    },
    Tool {
        name: "add_lead",
        description: "Add a lead that did not arrive by phone, such as a form submission or a walk-in.",
        changes: true,
        idempotent: false,
        schema: || {
            json!({"type": "object", "additionalProperties": false, "required": ["name", "contact", "source"], "properties": {
                "name": {"type": "string"},
                "contact": {"type": "string", "description": "Phone or email"},
                "source": {"type": "string"},
                "campaign": {"type": "string"},
                "notes": {"type": "string"},
                "external_id": {"type": "string", "description": "Your ID for this lead; repeating it will not create a duplicate"}
            }})
        },
    },
    Tool {
        name: "qualify_lead",
        description: "Mark a lead qualified, unqualified, spam or unreviewed. Notes are kept unless replaced.",
        changes: true,
        idempotent: true,
        schema: || {
            json!({"type": "object", "additionalProperties": false, "required": ["lead_id", "qualification"], "properties": {
                "lead_id": lead_id(),
                "qualification": {"type": "string", "enum": ["qualified", "unqualified", "spam", "unreviewed"]},
                "notes": {"type": "string", "description": "Replaces the lead's notes"}
            }})
        },
    },
    Tool {
        name: "record_outcome",
        description: "Record a booked job (with its value in USD) or cancel one. The reference, such as a job or invoice number, identifies the booking; reusing it revises that booking.",
        changes: true,
        idempotent: true,
        schema: || {
            json!({"type": "object", "additionalProperties": false, "required": ["lead_id", "reference", "status"], "properties": {
                "lead_id": lead_id(),
                "reference": {"type": "string", "description": "Job or invoice number"},
                "status": {"type": "string", "enum": ["booked", "canceled"]},
                "amount": {"type": "string", "description": "USD, e.g. 1250.00. Required for a new booking; defaults to the existing amount"}
            }})
        },
    },
];

const SETUP_PROMPT: &str = "Set up VisiConvert call tracking for this business.

1. Call get_status. If the chosen provider (Twilio or SignalWire) is not connected, ask the owner to connect it in the workspace UI. Scoped API keys cannot change provider credentials. Self-hosted operators using a legacy operator key can also run `visiconvert connect twilio --account-sid AC… --token-stdin` themselves. Never ask them to paste credentials into this conversation.
2. Ask which marketing sources and campaigns they want to track (for example Google Ads, Facebook, yard signs, the van), and which business line calls should ring through to.
3. Choose the phone provider with the person. For SignalWire, have the owner connect its Space, Project ID, API token, and separate signing key in the workspace UI; never request these secrets in chat. Call list_signalwire_numbers or list_twilio_numbers as appropriate. Propose one untracked number per campaign and confirm the plan with the person. If there are not enough numbers, tell them how many more to obtain from their provider; buying is not available from these tools yet.
4. For each confirmed pair, call add_tracking_number with provider set to twilio or signalwire.
5. Give the person the voice URL and status URL from the results, and the matching provider console steps for each number (SignalWire uses cXML handling). These have to be set by hand for now.
6. Ask them to call one tracking number from their own phone, then call list_leads with since 15m and confirm the call arrived with the right source.
7. Summarise what is tracked, and anything left to do.";

/// The reference page for the docs site, from the same table `tools/list` serves.
pub fn reference() -> String {
    let tools = TOOLS
        .iter()
        .map(|tool| crate::reference::ToolDoc {
            name: tool.name,
            description: tool.description,
            changes: tool.changes,
            schema: (tool.schema)(),
        })
        .collect();
    crate::reference::mcp(tools, SETUP_PROMPT)
}

impl Server {
    pub fn new(client: Result<Client, String>, allow_changes: bool) -> Self {
        Self {
            client,
            allow_changes,
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(message) => self.handle(message).await,
                Err(_) => Some(error_reply(Value::Null, -32700, "Parse error")),
            };
            if let Some(reply) = reply {
                stdout.write_all(format!("{reply}\n").as_bytes()).await?;
                stdout.flush().await?;
            }
        }
        Ok(())
    }

    /// Handles one JSON-RPC message. Notifications get no reply.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned()?;
        let method = message["method"].as_str().unwrap_or("");
        let params = &message["params"];
        let result = match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": self.tools()})),
            "tools/call" => Ok(self.call(params).await),
            "prompts/list" => Ok(json!({"prompts": [{
                "name": "set_up_call_tracking",
                "description": "Walk through connecting numbers to campaigns and confirming a test call."
            }]})),
            "prompts/get" if params["name"] == "set_up_call_tracking" => Ok(json!({
                "description": "Set up call tracking",
                "messages": [{"role": "user", "content": {"type": "text", "text": SETUP_PROMPT}}]
            })),
            "prompts/get" => Err((-32602, "Unknown prompt")),
            _ => Err((-32601, "Method not found")),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, text)) => error_reply(id, code, text),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params["protocolVersion"].as_str().unwrap_or("");
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == requested)
            .unwrap_or(&PROTOCOL_VERSIONS[0]);
        let mode = if self.allow_changes {
            "Tools can read and change this workspace."
        } else {
            "Tools are read-only. Restart with `visiconvert mcp --allow-changes` to add numbers, leads, qualifications and bookings."
        };
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {}, "prompts": {}},
            "serverInfo": {"name": "visiconvert", "version": env!("CARGO_PKG_VERSION")},
            "instructions": format!("VisiConvert call tracking and lead attribution. {mode} Call get_status first.")
        })
    }

    fn tools(&self) -> Vec<Value> {
        TOOLS
            .iter()
            .filter(|tool| self.allow_changes || !tool.changes)
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": (tool.schema)(),
                    "annotations": {
                        "readOnlyHint": !tool.changes,
                        "destructiveHint": false,
                        "idempotentHint": tool.idempotent,
                        "openWorldHint": false
                    }
                })
            })
            .collect()
    }

    async fn call(&self, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or("");
        let empty = json!({});
        let args = params.get("arguments").unwrap_or(&empty);
        let outcome = match TOOLS.iter().find(|tool| tool.name == name) {
            None => Err(anyhow!("Unknown tool {name}")),
            Some(tool) if tool.changes && !self.allow_changes => Err(anyhow!(
                "{name} changes data. Restart the server with `visiconvert mcp --allow-changes` to use it."
            )),
            Some(_) => match &self.client {
                Err(message) => Err(anyhow!("{message}")),
                Ok(client) => dispatch(client, name, args).await,
            },
        };
        match outcome {
            Ok(value) => json!({
                "content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}],
                "isError": false
            }),
            Err(error) => {
                json!({"content": [{"type": "text", "text": error.to_string()}], "isError": true})
            }
        }
    }
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn text(args: &Value, key: &str) -> Option<String> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn required(args: &Value, key: &str) -> Result<String> {
    text(args, key).ok_or_else(|| anyhow!("{key} is required"))
}

async fn dispatch(client: &Client, name: &str, args: &Value) -> Result<Value> {
    Ok(match name {
        "get_status" => serde_json::to_value(ops::status(client).await?)?,
        "list_numbers" => {
            json!({"numbers": client.numbers().await?.into_iter().filter(|n| !n.is_demo).collect::<Vec<_>>()})
        }
        "list_twilio_numbers" => ops::available_numbers(client).await?,
        "list_signalwire_numbers" => {
            ops::available_numbers_for(client, convert_domain::PhoneProvider::Signalwire).await?
        }
        "list_leads" => {
            let filter = ops::LeadFilter {
                since: text(args, "since")
                    .map(|s| {
                        ops::parse_since(
                            &s,
                            chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()),
                        )
                    })
                    .transpose()?,
                source: text(args, "source"),
                qualification: text(args, "qualification"),
                include_demo: args["include_demo"].as_bool().unwrap_or(false),
                limit: Some(args["limit"].as_u64().unwrap_or(50).clamp(1, 500) as usize),
            };
            let leads: Vec<Value> = ops::list_leads(client, &filter)
                .await?
                .into_iter()
                .map(|l| {
                    json!({
                        "id": l.id, "created_at": l.created_at, "name": l.name, "contact": l.contact,
                        "channel": l.channel, "source": l.source, "campaign": l.campaign,
                        "qualification": l.qualification, "notes": l.notes, "call_status": l.call_status,
                        "duration_seconds": l.duration_seconds, "tracking_number": l.tracking_number,
                        "booked": ops::money(l.booked_minor)
                    })
                })
                .collect();
            json!({"count": leads.len(), "leads": leads})
        }
        "get_lead_outcomes" => ops::outcomes(client, &required(args, "lead_id")?).await?,
        "source_report" => ops::source_report(client).await?,
        "add_tracking_number" => {
            ops::add_number(
                client,
                ops::AddNumber {
                    provider: text(args, "provider")
                        .unwrap_or_else(|| "twilio".into())
                        .parse()?,
                    phone_number: required(args, "phone_number")?,
                    forward_to: required(args, "forward_to")?,
                    source: required(args, "source")?,
                    campaign: required(args, "campaign")?,
                    label: text(args, "label"),
                },
            )
            .await?
        }
        "add_lead" => {
            let id = ops::add_lead(
                client,
                convert_domain::NewLead {
                    external_id: text(args, "external_id").unwrap_or_default(),
                    name: required(args, "name")?,
                    contact: required(args, "contact")?,
                    source: required(args, "source")?,
                    campaign: text(args, "campaign").unwrap_or_default(),
                    notes: text(args, "notes").unwrap_or_default(),
                },
            )
            .await?;
            json!({"id": id})
        }
        "qualify_lead" => {
            let qualification = ops::parse_qualification(&required(args, "qualification")?)?;
            ops::qualify(
                client,
                &required(args, "lead_id")?,
                qualification,
                text(args, "notes"),
            )
            .await?
        }
        "record_outcome" => {
            let status = match required(args, "status")?.as_str() {
                "booked" => ops::OutcomeStatus::Booked,
                "canceled" => ops::OutcomeStatus::Canceled,
                _ => bail!("status must be booked or canceled"),
            };
            let amount = text(args, "amount")
                .map(|a| ops::parse_money(&a))
                .transpose()?;
            ops::record_outcome(
                client,
                &required(args, "lead_id")?,
                &required(args, "reference")?,
                amount,
                status,
            )
            .await?
        }
        _ => bail!("Unknown tool {name}"),
    })
}
