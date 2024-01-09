use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use crusader_lib::file_format::RawResult;
use crusader_lib::protocol;
use crusader_lib::test::{Config, PlotConfig};

#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(clap::Args)]
struct PlotArgs {
    #[clap(long)]
    plot_transferred: bool,
    #[clap(long)]
    plot_split_bandwidth: bool,
    #[clap(long)]
    plot_width: Option<u64>,
    #[clap(long)]
    plot_height: Option<u64>,
}

impl PlotArgs {
    fn config(&self) -> PlotConfig {
        PlotConfig {
            transferred: self.plot_transferred,
            split_bandwidth: self.plot_split_bandwidth,
            width: self.plot_width,
            height: self.plot_height,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    Serve {
        #[clap(long, default_value_t = protocol::PORT)]
        port: u16,
    },
    Test {
        server: String,
        #[clap(long)]
        download: bool,
        #[clap(long)]
        upload: bool,
        #[clap(long)]
        both: bool,
        #[clap(long, default_value_t = protocol::PORT)]
        port: u16,
        #[clap(long, default_value_t = 16)]
        streams: u64,
        #[clap(long, default_value_t = 0.0, value_name = "SECONDS")]
        stream_stagger: f64,
        #[clap(long, default_value_t = 5.0, value_name = "SECONDS")]
        load_duration: f64,
        #[clap(long, default_value_t = 1.0, value_name = "SECONDS")]
        grace_duration: f64,
        #[clap(long, default_value_t = 5, value_name = "MILLISECONDS")]
        latency_sample_rate: u64,
        #[clap(long, default_value_t = 20, value_name = "MILLISECONDS")]
        bandwidth_sample_rate: u64,
        #[clap(flatten)]
        plot: PlotArgs,
    },
    Plot {
        data: PathBuf,
        #[clap(flatten)]
        plot: PlotArgs,
    },
}

fn main() {
    env_logger::init();

    let cli = Cli::parse();

    crusader_lib::plot::register_fonts();

    match &cli.command {
        &Commands::Test {
            ref server,
            download,
            upload,
            both,
            bandwidth_sample_rate,
            latency_sample_rate,
            ref plot,
            port,
            streams,
            stream_stagger,
            grace_duration,
            load_duration,
        } => {
            let mut config = Config {
                port,
                streams,
                stream_stagger: Duration::from_secs_f64(stream_stagger),
                grace_duration: Duration::from_secs_f64(grace_duration),
                load_duration: Duration::from_secs_f64(load_duration),
                download: true,
                upload: true,
                both: true,
                ping_interval: Duration::from_millis(latency_sample_rate),
                bandwidth_interval: Duration::from_millis(bandwidth_sample_rate),
            };

            if download || upload || both {
                config.download = download;
                config.upload = upload;
                config.both = both;
            }

            crusader_lib::test::test(config, plot.config(), server);
        }
        Commands::Serve { port } => {
            crusader_lib::serve::serve(*port);
        }
        Commands::Plot { data, plot } => {
            let result = RawResult::load(data).expect("Unable to load data");
            let file = crusader_lib::plot::save_graph(
                &plot.config(),
                &result.to_test_result(),
                data.file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("plot"),
            );
            println!("Saved plot as {}", file);
        }
    }
}
