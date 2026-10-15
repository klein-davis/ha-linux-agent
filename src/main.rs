use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ha_stat::entities::Source;
use ha_stat::{builtins, config, custom, mqtt};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ha-stat",
    version,
    about = "Expose laptop/system state and controls to Home Assistant over MQTT"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon: connect to MQTT, publish discovery, poll entities, and
    /// handle incoming commands, per a declarative TOML config.
    Run {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Parse and validate a config file without connecting to MQTT.
    Check {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Run a single ad-hoc read (and optionally publish it once) with no
    /// config file at all -- for quick one-off checks from the shell.
    Oneshot {
        /// Name of a parameterless builtin (cpu_usage, memory_usage, uptime, ...).
        #[arg(long, conflicts_with = "command")]
        builtin: Option<String>,
        /// Shell command to run instead of a builtin; stdout is the value.
        #[arg(long, conflicts_with = "builtin")]
        command: Option<String>,
        /// How to interpret `command`'s stdout: number, text, or bool.
        #[arg(long, default_value = "number")]
        value_type: String,
        /// Path argument for the disk_usage builtin.
        #[arg(long)]
        path: Option<String>,
        /// If set, publish the result to this MQTT topic instead of printing it.
        #[arg(long)]
        publish_topic: Option<String>,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 1883)]
        port: u16,
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        retain: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Run { config } => {
            let raw = config::RawConfig::from_file(&config)?;
            mqtt::run_daemon(raw).await
        }
        Commands::Check { config } => cmd_check(&config),
        Commands::Oneshot {
            builtin,
            command,
            value_type,
            path,
            publish_topic,
            host,
            port,
            username,
            password,
            retain,
        } => {
            cmd_oneshot(
                builtin,
                command,
                value_type,
                path,
                publish_topic,
                host,
                port,
                username,
                password,
                retain,
            )
            .await
        }
    }
}

fn cmd_check(config_path: &Path) -> Result<()> {
    let raw = config::RawConfig::from_file(config_path)?;
    let entities = raw.build_entities()?;
    println!("device: {} (id: {})", raw.device.name, raw.device_id());
    for e in &entities {
        println!(
            "  [{:<13}] {:<24} readable={:<5} writable={}",
            e.component.as_str(),
            e.id,
            e.is_readable(),
            e.is_writable()
        );
    }
    println!("OK: {} entities", entities.len());
    Ok(())
}

fn oneshot_builtin_source(name: &str, path: &Option<String>) -> Result<Box<dyn Source>> {
    use builtins::{battery, brightness, system};
    Ok(match name {
        "cpu_usage" => Box::new(system::CpuUsageSource::new()),
        "memory_usage" => Box::new(system::MemoryUsageSource),
        "swap_usage" => Box::new(system::SwapUsageSource),
        "uptime" => Box::new(system::UptimeSource),
        "load1" => Box::new(system::LoadAverageSource { column: 0 }),
        "load5" => Box::new(system::LoadAverageSource { column: 1 }),
        "load15" => Box::new(system::LoadAverageSource { column: 2 }),
        "process_count" => Box::new(system::ProcessCountSource),
        "battery_percent" => Box::new(battery::BatteryPercentSource),
        "charging" => Box::new(battery::ChargingSource),
        "ac_connected" => Box::new(battery::AcConnectedSource),
        "brightness" => Box::new(brightness::BrightnessController { device: None }),
        "disk_usage" => {
            let path = path.clone().context("--path is required for builtin disk_usage")?;
            Box::new(system::DiskUsageSource { path })
        }
        other => bail!("unknown builtin '{other}' (try: cpu_usage, memory_usage, swap_usage, uptime, load1, load5, load15, process_count, battery_percent, charging, ac_connected, brightness, disk_usage)"),
    })
}

#[allow(clippy::too_many_arguments)]
async fn cmd_oneshot(
    builtin: Option<String>,
    command: Option<String>,
    value_type: String,
    path: Option<String>,
    publish_topic: Option<String>,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    retain: bool,
) -> Result<()> {
    let source: Box<dyn Source> = match (&builtin, &command) {
        (Some(name), None) => oneshot_builtin_source(name, &path)?,
        (None, Some(cmd)) => {
            let kind = match value_type.as_str() {
                "number" => custom::ValueKind::Number,
                "text" => custom::ValueKind::Text,
                "bool" => custom::ValueKind::Bool,
                other => bail!("invalid --value-type '{other}' (expected number, text, or bool)"),
            };
            Box::new(custom::CommandSource {
                command: cmd.clone(),
                kind,
            })
        }
        _ => bail!("oneshot requires exactly one of --builtin or --command"),
    };

    let value = source.read().await.context("read failed")?;
    let payload = value.as_mqtt_payload();

    match publish_topic {
        None => println!("{payload}"),
        Some(topic) => {
            let client_id = format!("ha-stat-oneshot-{}", std::process::id());
            let mut opts = rumqttc::MqttOptions::new(client_id, host, port);
            if let (Some(u), Some(p)) = (username, password) {
                opts.set_credentials(u, p);
            }
            opts.set_keep_alive(Duration::from_secs(5));
            let (client, mut eventloop) = rumqttc::AsyncClient::new(opts, 10);
            client
                .publish(&topic, rumqttc::QoS::AtLeastOnce, retain, payload.clone())
                .await
                .context("failed to publish")?;
            // Drive the event loop until the publish is acknowledged.
            for _ in 0..20 {
                match eventloop.poll().await {
                    Ok(rumqttc::Event::Outgoing(rumqttc::Outgoing::Publish(_)))
                    | Ok(rumqttc::Event::Incoming(rumqttc::Packet::PubAck(_))) => break,
                    Ok(_) => continue,
                    Err(e) => bail!("mqtt connection error: {e}"),
                }
            }
            println!("published '{payload}' to {topic}");
        }
    }
    Ok(())
}
