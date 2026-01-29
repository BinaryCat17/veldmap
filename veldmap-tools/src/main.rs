mod copernicus;
mod gui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use crate::copernicus::CopernicusSource;
use veldmap_geo_math::create_geo_math;
use gui::VeldMapToolsGui;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the world dataset (Low Res GLO-90)
    InitWorld {
        /// Path to the data directory (root of the pyramid)
        #[arg(short, long, default_value = "./data/dem")]
        output_dir: PathBuf,
    },
    /// Download a high-res region (GLO-30)
    AddRegion {
        /// Bounding box in format "min_lon,min_lat,max_lon,max_lat"
        /// Example for European Russia: "26,41,60,70"
        #[arg(long)]
        bbox: String,
        
        /// Path to the data directory
        #[arg(short, long, default_value = "./data/dem")]
        output_dir: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    // Load environment variables from .env file if it exists
    dotenv::dotenv().ok();
    
    let args: Vec<String> = std::env::args().collect();
    
    // If no arguments (other than binary name), launch GUI
    if args.len() <= 1 {
        return iced::application("VeldMap Tools - Copernicus Explorer", VeldMapToolsGui::update, VeldMapToolsGui::view)
            .run_with(VeldMapToolsGui::new)
            .map_err(|e| anyhow::anyhow!("GUI error: {}", e));
    }

    // Otherwise, handle CLI
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        handle_cli(cli).await
    })
}

async fn handle_cli(cli: Cli) -> anyhow::Result<()> {
    // Create the math module instance via factory
    let _geo_math = create_geo_math();

    if let Some(command) = cli.command {
        match command {
            Commands::InitWorld { output_dir } => {
                println!("Initializing world map in {:?}", output_dir);
                // TODO: Implement GLO-90 download logic
                println!("(Simulation) Downloading global GLO-90 data...");
            }
            Commands::AddRegion { bbox, output_dir } => {
                println!("Adding high-res region {} to {:?}", bbox, output_dir);
                
                let parts: Vec<&str> = bbox.split(',').collect();
                if parts.len() != 4 {
                    anyhow::bail!("Invalid bbox format. Expected: min_lon,min_lat,max_lon,max_lat");
                }
                
                let min_lon: f64 = parts[0].parse()?;
                let min_lat: f64 = parts[1].parse()?;
                let max_lon: f64 = parts[2].parse()?;
                let max_lat: f64 = parts[3].parse()?;

                let start_lon = min_lon.floor() as i32;
                let end_lon = max_lon.ceil() as i32;
                let start_lat = min_lat.floor() as i32;
                let end_lat = max_lat.ceil() as i32;

                println!("Grid range: Lat [{}..{}), Lon [{}..{})", start_lat, end_lat, start_lon, end_lon);
                let source = CopernicusSource::new().await?;
                let source_dir = output_dir.join("source");
                std::fs::create_dir_all(&source_dir)?;

                for lat in start_lat..end_lat {
                    for lon in start_lon..end_lon {
                        // Calculate filename for local storage
                        let lat_char = if lat >= 0 { 'N' } else { 'S' };
                        let lon_char = if lon >= 0 { 'E' } else { 'W' };
                        let lat_str = format!("{}{:02}_00", lat_char, lat.abs());
                        let lon_str = format!("{}{:03}_00", lon_char, lon.abs());
                        let filename = format!("Copernicus_DSM_COG_10_{}_{}_DEM.tif", lat_str, lon_str);
                        let file_path = source_dir.join(&filename);

                        if file_path.exists() {
                            println!("Skipping existing: {}", filename);
                            continue;
                        }

                        // Try to download/locate the tile
                        match source.download_tile(lat, lon).await {
                            Ok(Some(bytes)) => {
                                std::fs::write(&file_path, bytes)?;
                                println!("Saved to {:?}", file_path);
                            },
                            Ok(None) => {
                                // Ocean or missing
                            },
                            Err(e) => {
                                eprintln!("Failed to download tile {}_{}: {}", lat, lon, e);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}