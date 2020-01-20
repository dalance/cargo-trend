mod db;
mod plotter;

use crate::db::Db;
use crate::plotter::Plotter;
use anyhow::{anyhow, Error};
use cargo_metadata::MetadataCommand;
use chrono::{Duration, Utc};
use directories::ProjectDirs;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use structopt::{clap, StructOpt};

// ---------------------------------------------------------------------------------------------------------------------
// Opt
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Debug, StructOpt)]
#[structopt(bin_name("cargo"))]
pub enum Opt {
    #[structopt(long_version(option_env!("LONG_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))))]
    #[structopt(setting(clap::AppSettings::ColoredHelp))]
    #[structopt(setting(clap::AppSettings::DeriveDisplayOrder))]
    Trend {
        /// Crates
        crates: Vec<String>,

        /// X size of output image
        #[structopt(value_name = "UINT", long = "xsize", default_value = "1200")]
        x_size: u32,

        /// Y size of output image
        #[structopt(value_name = "UINT", long = "ysize", default_value = "800")]
        y_size: u32,

        /// File path of output image
        #[structopt(
            value_name = "PATH",
            short = "o",
            long = "output",
            default_value = "trend.svg"
        )]
        output: PathBuf,

        /// File path of Cargo.toml
        #[structopt(value_name = "PATH", long = "manifest-path")]
        manifest_path: Option<PathBuf>,

        /// Update db
        #[structopt(value_name = "PATH", short = "u", long = "update")]
        update: Option<PathBuf>,

        /// Branch of crates.io-index
        #[structopt(value_name = "BRANCH", short = "b", long = "branch")]
        branch: Option<String>,

        /// Plot fraction of crates.io
        #[structopt(long = "relative")]
        relative: bool,

        /// Plot transitive dependents
        #[structopt(long = "transitive")]
        transitive: bool,

        /// The most trending crates
        #[structopt(long = "top")]
        top: Option<usize>,

        /// Duration by week
        #[structopt(long = "duration")]
        duration: Option<i64>,
    },
}

// ---------------------------------------------------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        let mut iter = e.chain();
        eprintln!("{}", iter.next().unwrap());
        for e in iter {
            eprintln!("  Caused by: {}", e);
        }

        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let opt = Opt::from_args();

    let (
        crates,
        x_size,
        y_size,
        output,
        manifest_path,
        update,
        branch,
        relative,
        transitive,
        top,
        duration,
    ) = match opt {
        Opt::Trend {
            crates,
            x_size,
            y_size,
            output,
            manifest_path,
            update,
            branch,
            relative,
            transitive,
            top,
            duration,
        } => (
            crates,
            x_size,
            y_size,
            output,
            manifest_path,
            update,
            branch,
            relative,
            transitive,
            top,
            duration,
        ),
    };

    if let Some(path) = update {
        let mut db = if path.exists() {
            Db::load(&path)?
        } else {
            Db::new()
        };
        db.update(branch)?;
        db.save(&path)?;

        return Ok(());
    }

    let base_dir = ProjectDirs::from("org", "dalance", "cargo-trend").ok_or(anyhow!("aa"))?;
    let data_dir = base_dir.data_dir();
    fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("db.gz");

    let latest_hash =
        reqwest::get("https://github.com/dalance/cargo-trend/raw/master/db_v2/db.gz.sha256")?
            .text()?;

    let current_hash = if db_path.exists() {
        let mut file = File::open(&db_path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        format!("{:x}", Sha256::digest(&buf))
    } else {
        String::from("")
    };

    if latest_hash != current_hash {
        let mut res =
            reqwest::get("https://github.com/dalance/cargo-trend/raw/master/db_v2/db.gz")?;
        let mut buf = Vec::new();
        res.read_to_end(&mut buf)?;
        let mut file = File::create(&db_path)?;
        file.write_all(&buf)?;
        file.flush()?;
    }

    let db = Db::load(&db_path)?;

    let start_date = if let Some(duration) = duration {
        Some((Utc::now() - Duration::weeks(duration)).date())
    } else {
        None
    };

    let targets = if let Some(top) = top {
        let mut trend = Vec::new();
        for (name, entries) in &db.map {
            let mut entry_oldest = None;
            for entry in entries {
                if let Some(start_date) = start_date {
                    if entry.time.date() < start_date {
                        entry_oldest = Some(entry.clone());
                    }
                } else {
                    entry_oldest = Some(entry.clone());
                }
            }
            let entry_newest = entries.last();

            if let Some(entry_oldest) = entry_oldest {
                if let Some(entry_newest) = entry_newest {
                    let (dep_oldest, dep_newest) = if transitive {
                        (
                            entry_oldest.transitive_dependents,
                            entry_newest.transitive_dependents,
                        )
                    } else {
                        (
                            entry_oldest.direct_dependents,
                            entry_newest.direct_dependents,
                        )
                    };

                    let (dep_oldest, dep_newest) = if relative {
                        (
                            (dep_oldest as f64 / entry_oldest.total_crates as f64 * 10000.0) as i64,
                            (dep_newest as f64 / entry_newest.total_crates as f64 * 10000.0) as i64,
                        )
                    } else {
                        (dep_oldest as i64, dep_newest as i64)
                    };
                    trend.push((dep_newest - dep_oldest, name));
                }
            }
        }

        trend.sort_by_key(|x| x.0);

        let mut ret = Vec::new();
        for _ in 0..top {
            if let Some(c) = trend.pop() {
                ret.push(c.1.clone());
            }
        }
        ret
    } else if crates.len() == 0 {
        let mut cmd = MetadataCommand::new();
        if let Some(path) = manifest_path {
            cmd.manifest_path(path);
        }
        let metadata = cmd.exec()?;

        let mut ret = Vec::new();
        for package in metadata.packages {
            if metadata
                .workspace_members
                .iter()
                .any(|x| x.repr == package.id.repr)
            {
                for dep in package.dependencies {
                    ret.push(String::from(dep.name));
                }
            }
        }
        ret
    } else {
        crates
    };

    let plotter = Plotter::new().size((x_size, y_size));
    plotter.plot(
        output,
        targets.as_slice(),
        &db,
        relative,
        transitive,
        start_date,
    )?;

    Ok(())
}
