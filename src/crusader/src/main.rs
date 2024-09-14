use anyhow::Context;
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
use std::process;
#[cfg(feature = "client")]
use {
    anyhow::anyhow,
    std::fs::OpenOptions,
    std::io::{BufWriter, Write},
    std::path::Path,
    std::time::Duration,
};

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
        #[arg(long, help = "Allow use and discovery as a peer")]
        peer: bool,
    },
    #[command(
        long_about = "Runs a test client against a specified server and saves the result to the current directory. \
        By default this does a download test, an upload test, and a test doing both download and upload while measuring the latency to the server"
    )]
    #[cfg(feature = "client")]
    Test {
        server: Option<String>,
        #[arg(long, help = "Run a download test")]
        download: bool,
        #[arg(long, help = "Run an upload test")]
        upload: bool,
        #[arg(long, help = "Run a test doing both download and upload")]
        bidirectional: bool,
        #[arg(
            long,
            long_help = "Run a test only measuring latency. The duration is specified by `grace_duration`"
        )]
        idle: bool,
        #[arg(long, default_value_t = protocol::PORT, help = "Specifies the TCP and UDP port used by the server")]
        port: u16,
        #[arg(
            long,
            default_value_t = 8,
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
            default_value_t = 10.0,
            value_name = "SECONDS",
            help = "The duration in which traffic is generated"
        )]
        load_duration: f64,
        #[arg(
            long,
            default_value_t = 2.0,
            value_name = "SECONDS",
            help = "The idle time between each test"
        )]
        grace_duration: f64,
        #[arg(long, default_value_t = 5, value_name = "MILLISECONDS")]
        latency_sample_interval: u64,
        #[arg(long, default_value_t = 60, value_name = "MILLISECONDS")]
        throughput_sample_interval: u64,
        #[command(flatten)]
        plot: PlotArgs,
        #[arg(
            long,
            long_help = "Specifies another server (peer) which will also measure the latency to the server independently of the client"
        )]
        latency_peer_address: Option<String>,
        #[arg(
            long,
            help = "Use another server (peer) which will also measure the latency to the server independently of the client"
        )]
        latency_peer: bool,
        #[arg(
            long,
            help = "The filename prefix used for the test result raw data and plot filenames"
        )]
        out_name: Option<String>,
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
    #[cfg(feature = "client")]
    #[command(about = "Converts a result file to JSON")]
    Export {
        data: PathBuf,
        #[arg(
            long,
            short('o'),
            help = "The path where the output JSON will be stored"
        )]
        output: Option<PathBuf>,
        #[arg(long, short('f'), help = "Overwrite the file if it exists")]
        force: bool,
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
            bidirectional,
            idle,
            throughput_sample_interval,
            latency_sample_interval,
            ref plot,
            port,
            streams,
            stream_stagger,
            grace_duration,
            load_duration,
            ref latency_peer_address,
            latency_peer,
            ref out_name,
        } => {
            let mut config = Config {
                port,
                streams,
                stream_stagger: Duration::from_secs_f64(stream_stagger),
                grace_duration: Duration::from_secs_f64(grace_duration),
                load_duration: Duration::from_secs_f64(load_duration),
                download: !idle,
                upload: !idle,
                bidirectional: !idle,
                ping_interval: Duration::from_millis(latency_sample_interval),
                throughput_interval: Duration::from_millis(throughput_sample_interval),
            };

            if download || upload || bidirectional {
                if idle {
                    println!("Cannot run `idle` test with a load test");
                    process::exit(1);
                }
                config.download = download;
                config.upload = upload;
                config.bidirectional = bidirectional;
            }

            crusader_lib::test::test(
                config,
                plot.config(),
                server.as_deref(),
                (latency_peer || latency_peer_address.is_some())
                    .then_some(latency_peer_address.as_deref()),
                out_name.as_deref().unwrap_or("test"),
            )
        }
        &Commands::Serve { port, peer } => crusader_lib::serve::serve(port, peer),

        #[cfg(feature = "client")]
        Commands::Remote { port } => crusader_lib::remote::run(*port),

        #[cfg(feature = "client")]
        Commands::Plot { data, plot } => {
            let result = RawResult::load(data).ok_or(anyhow!("Unable to load data"))?;
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
        #[cfg(feature = "client")]
        Commands::Export {
            data,
            output,
            force,
        } => {
            let result = RawResult::load(data).ok_or(anyhow!("Unable to load data"))?;
            let output = output
                .clone()
                .unwrap_or_else(|| data.with_extension("json"));
            let file = OpenOptions::new()
                .create_new(!*force)
                .create(*force)
                .truncate(true)
                .write(true)
                .open(output)
                .context("Failed to create output file")?;
            let mut file = BufWriter::new(file);
            serde_json::to_writer_pretty(&mut file, &result).context("Failed to serialize data")?;
            file.flush().context("Failed to flush output")?;

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
