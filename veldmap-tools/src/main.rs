mod copernicus;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use crate::copernicus::CopernicusSource;
use veldmap_geo_math::create_geo_math;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    
    // Create the math module instance via factory
    let geo_math = create_geo_math();

    match &cli.command {
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
            let source = CopernicusSource::new().await;

            for lat in start_lat..end_lat {
                for lon in start_lon..end_lon {
                    // Demonstrate using the shared math module
                    // Calculate which Web Mercator tile (at z=10) this 1x1 degree cell center belongs to
                    let center_lat = lat as f64 + 0.5;
                    let center_lon = lon as f64 + 0.5;
                    let tile_id = geo_math.lat_lon_to_tile(center_lat, center_lon, 10);
                    
                    println!("Processing 1x1 cell at {}, {}. Maps to tile {:?} (z10)", lat, lon, tile_id);

                    // Try to download/locate the tile
                    let _ = source.download_tile(lat, lon).await;
                }
            }
        }
    }

    Ok(())
}