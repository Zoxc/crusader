use clap::{Parser, Subcommand};
use clap_num::si_number;
#[cfg(feature = "client")]
use crusader_lib::file_format::RawResult;
#[cfg(feature = "client")]
use crusader_lib::test::PlotConfig;
use crusader_lib::{protocol, version};
#[cfg(feature = "client")]
use crusader_lib::{with_time, Config};
#[cfg(feature = "client")]
use std::path::PathBuf;
#[cfg(feature = "client")]
use std::time::Duration;
use std::{path::Path, process};

#[derive(Parser)]
#[command(version = version())]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Args)]
struct PlotArgs {
    #[arg(long, help = "Plot transferred bytes")]
    plot_transferred: bool,
    #[arg(long, help = "Plot upload and download separately and plot streams")]
    plot_split_throughput: bool,
    #[arg(long, value_parser=si_number::<u64>, value_name = "BPS",
        long_help = "Sets the axis for throughput to at least this value. \
            SI units are supported so `100M` would specify 100 Mbps")]
    plot_max_throughput: Option<u64>,
    #[arg(
        long,
        value_name = "MILLISECONDS",
        help = "Sets the axis for latency to at least this value"
    )]
    plot_max_latency: Option<u64>,
    #[arg(long, value_name = "PIXELS")]
    plot_width: Option<u64>,
    #[arg(long, value_name = "PIXELS")]
    plot_height: Option<u64>,
    #[arg(long)]
    plot_title: Option<String>,
}

impl PlotArgs {
    #[cfg(feature = "client")]
    fn config(&self) -> PlotConfig {
        PlotConfig {
            transferred: self.plot_transferred,
            split_throughput: self.plot_split_throughput,
            max_throughput: self.plot_max_throughput,
            max_latency: self.plot_max_latency,
            width: self.plot_width,
            height: self.plot_height,
            title: self.plot_title.clone(),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Runs the server")]
    Serve {
        #[arg(long, default_value_t = protocol::PORT, help = "Specifies the TCP and UDP port used by the server")]
        port: u16,
    },
    #[command(
        long_about = "Runs a test client against a specified server and saves the result to the current directory. \
        By default this does a download test, an upload test, and a test doing both download and upload while measuring the latency to the server"
    )]
    #[cfg(feature = "client")]
    Test {
        server: String,
        #[arg(long, help = "Run a download test")]
        download: bool,
        #[arg(long, help = "Run an upload test")]
        upload: bool,
        #[arg(long, help = "Run a test doing both download and upload")]
        both: bool,
        #[arg(
            long,
            long_help = "Run a test only measuring latency. The duration is specified by `grace_duration`"
        )]
        idle: bool,
        #[arg(long, default_value_t = protocol::PORT, help = "Specifies the TCP and UDP port used by the server")]
        port: u16,
        #[arg(
            long,
            default_value_t = 16,
            help = "The number of TCP connections used to generate traffic in a single direction"
        )]
        streams: u64,
        #[arg(
            long,
            default_value_t = 0.0,
            value_name = "SECONDS",
            help = "The delay between the start of each stream"
        )]
        stream_stagger: f64,
        #[arg(
            long,
            default_value_t = 5.0,
            value_name = "SECONDS",
            help = "The duration in which traffic is generated"
        )]
        load_duration: f64,
        #[arg(
            long,
            default_value_t = 1.0,
            value_name = "SECONDS",
            help = "The idle time between each test"
        )]
        grace_duration: f64,
        #[arg(long, default_value_t = 5, value_name = "MILLISECONDS")]
        latency_sample_rate: u64,
        #[arg(long, default_value_t = 20, value_name = "MILLISECONDS")]
        throughput_sample_rate: u64,
        #[command(flatten)]
        plot: PlotArgs,
        #[arg(
            long,
            long_help = "Specifies another server (peer) which will also measure the latency to the server independently of the client"
        )]
        latency_peer: Option<String>,
    },
    #[cfg(feature = "client")]
    #[command(about = "Plots a previous result")]
    Plot {
        data: PathBuf,
        #[command(flatten)]
        plot: PlotArgs,
    },
    #[cfg(feature = "client")]
    #[command(about = "Allows the client to be controlled over a web server")]
    Remote {
        #[arg(
            long,
            default_value_t = protocol::PORT + 1,
            help = "Specifies the HTTP port used by the server"
        )]
        port: u16,
    },
}

fn run() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    match &cli.command {
        #[cfg(feature = "client")]
        &Commands::Test {
            ref server,
            download,
            upload,
            both,
            idle,
            throughput_sample_rate,
            latency_sample_rate,
            ref plot,
            port,
            streams,
            stream_stagger,
            grace_duration,
            load_duration,
            ref latency_peer,
        } => {
            let mut config = Config {
                port,
                streams,
                stream_stagger: Duration::from_secs_f64(stream_stagger),
                grace_duration: Duration::from_secs_f64(grace_duration),
                load_duration: Duration::from_secs_f64(load_duration),
                download: !idle,
                upload: !idle,
                both: !idle,
                ping_interval: Duration::from_millis(latency_sample_rate),
                throughput_interval: Duration::from_millis(throughput_sample_rate),
            };

            if download || upload || both {
                if idle {
                    println!("Cannot run `idle` test with a load test");
                    process::exit(1);
                }
                config.download = download;
                config.upload = upload;
                config.both = both;
            }

            crusader_lib::test::test(config, plot.config(), server, latency_peer.as_deref())
        }
        Commands::Serve { port } => crusader_lib::serve::serve(*port),

        #[cfg(feature = "client")]
        Commands::Remote { port } => crusader_lib::remote::run(*port),

        #[cfg(feature = "client")]
        Commands::Plot { data, plot } => {
            let result = RawResult::load(data).expect("Unable to load data");
            let root = data.parent().unwrap_or(Path::new(""));
            let file = crusader_lib::plot::save_graph(
                &plot.config(),
                &result.to_test_result(),
                data.file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("plot"),
                data.parent().unwrap_or(Path::new("")),
            )?;
            println!(
                "{}",
                with_time(&format!("Saved plot as {}", root.join(file).display()))
            );
            Ok(())
        }
    }
}

fn main() {
    env_logger::init();

    #[cfg(feature = "client")]
    crusader_lib::plot::register_fonts();

    if let Err(error) = run() {
        println!("Error: {:?}", error);
        process::exit(1);
    }
}
