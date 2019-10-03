use chrono::serde::ts_seconds;
use chrono::{DateTime, TimeZone, Utc};
use crates_index::Index;
use failure::Error;
use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};
use git2::{BranchType, Repository, ResetType};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::tempdir;

#[derive(Serialize, Deserialize, Debug)]
pub struct Db {
    pub update: DateTime<Utc>,
    pub map: HashMap<String, Vec<Entry>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Entry {
    #[serde(with = "ts_seconds")]
    pub time: DateTime<Utc>,
    pub dependents: u64,
}

impl Db {
    pub fn new() -> Db {
        Db {
            update: Utc.timestamp(0, 0),
            map: HashMap::new(),
        }
    }

    pub fn load<T: AsRef<Path>>(path: T) -> Result<Db, Error> {
        let file = File::open(path)?;
        let mut gz = GzDecoder::new(file);
        let mut buf = Vec::new();
        gz.read_to_end(&mut buf)?;
        let db = serde_json::from_str(&String::from_utf8(buf)?)?;
        Ok(db)
    }

    pub fn save<T: AsRef<Path>>(&self, path: T) -> Result<(), Error> {
        let encoded: Vec<u8> = serde_json::to_string(self)?.into_bytes();
        let file = File::create(&path)?;
        let mut gz = GzBuilder::new().write(file, Compression::default());
        gz.write_all(&encoded)?;
        gz.finish()?;

        let mut file = File::open(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        let hash = Sha256::digest(&buf);
        let path = path.as_ref().with_extension("gz.sha256");
        let mut file = File::create(path)?;
        file.write_all(&format!("{:x}", hash).as_bytes())?;
        file.flush()?;

        Ok(())
    }

    pub fn update(&mut self, branch: Option<String>) -> Result<(), Error> {
        let url = "https://github.com/rust-lang/crates.io-index.git";
        let dir = tempdir()?;

        let repo = Repository::clone(url, &dir)?;
        repo.checkout_head(None)?;
        if let Some(branch) = branch {
            let master = repo.find_branch(&branch, BranchType::Remote)?;
            let master = master.get().peel_to_commit()?;
            let master = repo.find_object(master.id(), None)?;
            repo.reset(&master, ResetType::Hard, None)?;
        }

        let mut revs = Vec::new();
        let mut revwalk = repo.revwalk()?;
        revwalk.push_head()?;
        let mut last = Utc.timestamp(0, 0);
        for oid in revwalk {
            let commit = repo.find_commit(oid?)?;
            let time = Utc.timestamp(commit.time().seconds(), 0);
            if last.date() != time.date() && time > self.update {
                revs.push((time, commit.id()));
                last = time;
            }
        }

        revs.reverse();

        for (time, id) in &revs {
            dbg!((time, id));
            let obj = repo.find_object(id.clone(), None)?;
            repo.reset(&obj, ResetType::Hard, None)?;

            let index = Index::new(&dir.path());
            let mut deps = HashMap::new();
            for c in index.crates() {
                for dep in c.latest_version().dependencies() {
                    let name = String::from(dep.name());
                    if let Some(cnt) = deps.get_mut(&name) {
                        *cnt = *cnt + 1;
                    } else {
                        deps.insert(name, 1);
                    }
                }
            }

            for (name, dependents) in &deps {
                let entry = Entry {
                    time: *time,
                    dependents: *dependents,
                };
                if let Some(entries) = self.map.get_mut(name) {
                    let last = &entries[entries.len() - 1];
                    if last.dependents != *dependents {
                        (*entries).push(entry);
                    }
                } else {
                    self.map.insert(name.clone(), vec![entry]);
                }
            }

            self.update = *time;
        }

        Ok(())
    }
}
