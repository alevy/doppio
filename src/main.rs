use std::{fs::File, io::{Read as _, Write}, path::PathBuf};

use clap::{Parser, Subcommand};


#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// does testing things
    Compile {
        /// lists test values
        #[arg(short, long)]
        output: PathBuf,

        source: PathBuf,
    },
}

fn main () -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile { output, source } => {
            let mut file = String::new();
            File::open(source)?.read_to_string(&mut file)?;
            let mut file = file.as_str();
            let journal = ledger::compile(&mut file)?;
            let mut output_file = File::create(output)?;
            serde_pickle::ser::to_writer(&mut output_file, &journal, serde_pickle::SerOptions::new().proto_v2())?;
        }
    }
    Ok(())
}
