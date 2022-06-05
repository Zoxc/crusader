use clap::{Parser, Subcommand};
use library::test2::Config;

#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Serve {
        #[clap(long, default_value_t = 30481)]
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
        #[clap(long, default_value_t = 30481)]
        port: u16,
        #[clap(long, default_value_t = 16)]
        streams: u64,
        #[clap(long, default_value_t = 5, value_name = "SECONDS")]
        load_duration: u64,
        #[clap(long, default_value_t = 1, value_name = "SECONDS")]
        grace_duration: u64,
        #[clap(long, default_value_t = 5, value_name = "MILLISECONDS")]
        latency_sample_rate: u64,
        #[clap(long, default_value_t = 20, value_name = "MILLISECONDS")]
        bandwidth_sample_rate: u64,
        #[clap(long)]
        plot_transferred: bool,
        #[clap(long)]
        plot_width: Option<u64>,
        #[clap(long)]
        plot_height: Option<u64>,
    },
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        &Commands::Test {
            ref server,
            download,
            upload,
            both,
            bandwidth_sample_rate,
            latency_sample_rate,
            plot_transferred,
            plot_width,
            plot_height,
            port,
            streams,
            grace_duration,
            load_duration,
        } => {
            let mut config = Config {
                port,
                streams,
                grace_duration,
                load_duration,
                download: true,
                upload: true,
                both: true,
                ping_interval: latency_sample_rate,
                bandwidth_interval: bandwidth_sample_rate,
                plot_transferred,
                plot_width,
                plot_height,
            };

            if download || upload || both {
                config.download = download;
                config.upload = upload;
                config.both = both;
            }

            library::test2::test(config, &server);
        }
        Commands::Serve { port } => {
            library::serve2::serve(*port);
        }
    }
}
