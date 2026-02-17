use std::{fs::File, io::{Read as _}, path::PathBuf};

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
    Balance {
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
            let output_file = File::create(output)?;
            let mut output_xz = xz::write::XzEncoder::new(output_file, 0);
            serde_pickle::ser::to_writer(&mut output_xz, &journal, Default::default())?;
        },
        Commands::Balance { source } => {
            let journal = if let Some("bki") = source.extension().and_then(|e| e.to_str()) {
                let input_xz = xz::read::XzDecoder::new(File::open(source)?);
                serde_pickle::de::from_reader(input_xz, Default::default())?
            } else {
                let mut file = String::new();
                File::open(source)?.read_to_string(&mut file)?;
                let mut file = file.as_str();
                ledger::compile(&mut file)?
            };
            println!("{}", journal.transactions.len());
        },
    }
    Ok(())
}
