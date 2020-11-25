mod db;
mod plotter;

use crate::db::Db;
use crate::plotter::Plotter;
use anyhow::{anyhow, Context, Error};
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
#[structopt(bin_name = "cargo")]
#[structopt(setting(clap::AppSettings::DisableHelpSubcommand))]
pub enum CargoOpt {
    #[structopt(long_version(option_env!("LONG_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))))]
    #[structopt(setting(clap::AppSettings::ColoredHelp))]
    #[structopt(setting(clap::AppSettings::DeriveDisplayOrder))]
    Trend(Opt),
}

#[derive(Debug, StructOpt)]
pub struct Opt {
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
    #[structopt(
            value_name = "N",
            long = "top-trend",
            conflicts_with_all = &["top-dependent", "top_transitive"]
        )]
    top_trend: Option<usize>,

    /// The most dependent crates
    #[structopt(value_name = "N", long = "top-dependent", conflicts_with_all = &["top_trend", "top_transitive"])]
    top_dependent: Option<usize>,

    /// The most transitive dependent crates
    #[structopt(value_name = "N", long = "top-transitive", conflicts_with_all = &["top_trend", "top_dependent"])]
    top_transitive: Option<usize>,

    /// Duration by week
    #[structopt(long = "duration")]
    duration: Option<i64>,
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
    let CargoOpt::Trend(opt) = CargoOpt::from_args();

    let mut transitive = opt.transitive;

    if let Some(path) = opt.update {
        let mut db = if path.exists() {
            Db::load(&path)?
        } else {
            Db::new()
        };
        db.update(opt.branch)?;
        db.save(&path)?;

        return Ok(());
    }

    let base_dir = ProjectDirs::from("org", "dalance", "cargo-trend")
        .ok_or_else(|| anyhow!("failed to find user directory"))?;
    let data_dir = base_dir.data_dir();
    fs::create_dir_all(data_dir).with_context(|| {
        format!(
            "failed to create data direcotry {}",
            data_dir.to_string_lossy()
        )
    })?;
    let db_path = data_dir.join("db.gz");

    let latest_hash = reqwest::blocking::get(
        "https://github.com/dalance/cargo-trend/raw/master/db_v2/db.gz.sha256",
    )?
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
        let mut res = reqwest::blocking::get(
            "https://github.com/dalance/cargo-trend/raw/master/db_v2/db.gz",
        )?;
        let mut buf = Vec::new();
        res.read_to_end(&mut buf)?;
        let mut file = File::create(&db_path)?;
        file.write_all(&buf)?;
        file.flush()?;
    }

    let db = Db::load(&db_path)?;

    let start_date = if let Some(duration) = opt.duration {
        Some((Utc::now() - Duration::weeks(duration)).date())
    } else {
        None
    };

    let targets = if let Some(top_trend) = opt.top_trend {
        let mut trend = Vec::new();
        for (name, entries) in &db.map {
            let mut entry_oldest = entries.first();
            for entry in entries {
                if let Some(start_date) = start_date {
                    if entry.time.date() < start_date {
                        entry_oldest = Some(entry);
                    }
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

                    let (dep_oldest, dep_newest) = if opt.relative {
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
        for _ in 0..top_trend {
            if let Some(c) = trend.pop() {
                ret.push(c.1.clone());
            }
        }
        ret
    } else if let Some(top_dependent) = opt.top_dependent {
        transitive = false;

        let mut trend = Vec::new();
        for (name, entries) in &db.map {
            if let Some(entry) = entries.last() {
                trend.push((entry.direct_dependents, name));
            }
        }

        trend.sort_by_key(|x| x.0);

        let mut ret = Vec::new();
        for _ in 0..top_dependent {
            if let Some(c) = trend.pop() {
                ret.push(c.1.clone());
            }
        }
        ret
    } else if let Some(top_transitive) = opt.top_transitive {
        transitive = true;

        let mut trend = Vec::new();
        for (name, entries) in &db.map {
            if let Some(entry) = entries.last() {
                trend.push((entry.transitive_dependents, name));
            }
        }

        trend.sort_by_key(|x| x.0);

        let mut ret = Vec::new();
        for _ in 0..top_transitive {
            if let Some(c) = trend.pop() {
                ret.push(c.1.clone());
            }
        }
        ret
    } else if opt.crates.is_empty() {
        let mut cmd = MetadataCommand::new();
        if let Some(path) = opt.manifest_path {
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
                    ret.push(dep.name);
                }
            }
        }
        ret
    } else {
        opt.crates
    };

    let plotter = Plotter::new().size((opt.x_size, opt.y_size));
    plotter.plot(
        opt.output,
        targets.as_slice(),
        &db,
        opt.relative,
        transitive,
        start_date,
    )?;

    Ok(())
}
