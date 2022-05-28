use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Serve,
    Test { server: String },
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Test { server } => {
            library::test::test(&server);
        }
        Commands::Serve => {
            library::serve::serve();
        }
    }
}
