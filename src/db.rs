use anyhow::Error;
use chrono::serde::ts_seconds;
use chrono::{DateTime, TimeZone, Utc};
use crates_index::{Crate, Dependency, GitIndex};
use dlhn::{Deserializer, Serializer};
use git2::{BranchType, Repository, ResetType};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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
pub struct DbHeader {
    pub update: DateTime<Utc>,
    pub hash: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DbChunk {
    pub data: Vec<(String, Entry)>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Entry {
    #[serde(with = "ts_seconds")]
    pub time: DateTime<Utc>,
    pub direct_dependents: u64,
    pub transitive_dependents: u64,
    pub total_crates: u64,
}

impl Db {
    pub fn new() -> Db {
        Db {
            update: Utc.timestamp_opt(0, 0).unwrap(),
            map: HashMap::new(),
        }
    }

    pub fn load<T: AsRef<Path>>(dir: T) -> Result<Db, Error> {
        let path = dir.as_ref().join("db.json");
        let mut file = File::open(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let header: DbHeader = serde_json::from_str(&String::from_utf8(buf)?)?;

        let mut db = Db {
            update: header.update,
            map: HashMap::new(),
        };

        for i in 0..header.hash.len() {
            let path = dir.as_ref().join(format!("db{}", i));
            let mut file = File::open(path)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            let mut buf = buf.as_slice();
            let mut deserializer = Deserializer::new(&mut buf);
            let chunk = DbChunk::deserialize(&mut deserializer)?;

            for (name, entry) in chunk.data {
                db.map
                    .entry(name)
                    .and_modify(|e| e.push(entry.clone()))
                    .or_insert(vec![entry]);
            }
        }

        Ok(db)
    }

    pub fn save<T: AsRef<Path>>(&self, dir: T) -> Result<(), Error> {
        let mut map: Vec<_> = self.map.iter().collect();
        map.sort_by_key(|x| x.0);

        let mut data = Vec::new();
        for (k, v) in map {
            for e in v {
                data.push((k.to_owned(), e.to_owned()));
            }
        }
        data.sort_by_key(|x| x.1.time);

        let mut hashes = Vec::new();
        let mut i = 0;
        while data.len() > 1000000 {
            let rest = data.split_off(1000000);

            let path = dir.as_ref().join(format!("db{}", i));
            let hash = write_chunk(&path, data.clone())?;
            hashes.push(hash);

            data = rest;
            i += 1;
        }

        let path = dir.as_ref().join(format!("db{}", i));
        let hash = write_chunk(&path, data.clone())?;
        hashes.push(hash);

        let header = DbHeader {
            update: self.update.to_owned(),
            hash: hashes,
        };
        let encoded: Vec<u8> = serde_json::to_string(&header)?.into_bytes();
        let path = dir.as_ref().join("db.json");
        let mut file = File::create(path)?;
        file.write_all(&encoded)?;
        file.flush()?;

        Ok(())
    }

    pub fn update(&mut self, branch: Option<String>) -> Result<(), Error> {
        let url = "https://github.com/rust-lang/crates.io-index.git";
        let dir = tempdir()?;

        let repo = Repository::clone(url, &dir)?;
        repo.checkout_head(None)?;
        if let Some(branch) = branch {
            let master = repo.find_branch(&format!("origin/{}", branch), BranchType::Remote)?;
            let master = master.get().peel_to_commit()?;
            let master = repo.find_object(master.id(), None)?;
            repo.reset(&master, ResetType::Hard, None)?;
        }

        let mut revs = Vec::new();
        let mut revwalk = repo.revwalk()?;
        revwalk.push_head()?;
        let mut last = Utc.timestamp_opt(0, 0).unwrap();
        for oid in revwalk {
            let commit = repo.find_commit(oid?)?;
            let time = Utc.timestamp_opt(commit.time().seconds(), 0).unwrap();
            if last.date_naive() != time.date_naive() && time > self.update {
                revs.push((time, commit.id()));
                last = time;
            }
        }

        revs.reverse();

        let total = revs.len();
        for (i, (time, id)) in revs.iter().enumerate() {
            println!("Update DB: {} {} ( {} / {} )", time, id, i + 1, total);
            let obj = repo.find_object(id.clone(), None)?;
            repo.reset(&obj, ResetType::Hard, None)?;

            let index = GitIndex::with_path(&dir.path(), crates_index::git::URL)?;
            let mut crates = HashMap::new();
            for c in index.crates() {
                crates.insert(String::from(c.name()), c);
            }

            let n = crates.len();
            let mut names: Vec<String> = crates.keys().cloned().collect();
            names.sort();
            let id_of: HashMap<&str, u32> = names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.as_str(), i as u32))
                .collect();
            let crates_by_id: Vec<&Crate> =
                names.iter().map(|nm| &crates[nm.as_str()]).collect();

            let mut deps: HashMap<String, (u64, u64)> = HashMap::new();
            let mut cache: Vec<Option<HashSet<u32>>> = vec![None; n];
            for root_id in 0..n as u32 {
                let c = crates_by_id[root_id as usize];
                let enabled_features = [String::from("default")];

                for dep in gather_dependencies(c, &VersionReq::STAR, &enabled_features) {
                    let dn = dep.crate_name();
                    deps.entry(String::from(dn)).or_insert((0, 0)).0 += 1;
                }

                let mut trace = HashSet::<u32>::new();
                trace.insert(root_id);
                let (transitive, looped) = gather_transitive(
                    root_id,
                    &VersionReq::STAR,
                    &enabled_features,
                    &mut trace,
                    &crates_by_id,
                    &id_of,
                    &mut cache,
                );

                // connect looped transitive
                for &l in &looped {
                    let l_cached = match &cache[l as usize] {
                        Some(s) => s.clone(),
                        None => continue,
                    };
                    for &t in &transitive {
                        if let Some(t_cached) = cache[t as usize].as_mut() {
                            if t_cached.contains(&l) {
                                for &x in &l_cached {
                                    t_cached.insert(x);
                                }
                            }
                        }
                    }
                }

                for &t in &transitive {
                    deps.entry(names[t as usize].clone())
                        .or_insert((0, 0))
                        .1 += 1;
                }
            }

            let total_crates = n as u64;
            for (name, (direct, transitive)) in &deps {
                let entry = Entry {
                    time: *time,
                    direct_dependents: *direct,
                    transitive_dependents: *transitive,
                    total_crates,
                };
                if let Some(entries) = self.map.get_mut(name) {
                    let last = &entries[entries.len() - 1];
                    if last.direct_dependents != *direct
                        || last.transitive_dependents != *transitive
                    {
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

    pub fn fetch<T: AsRef<Path>>(dir: T) -> Result<(), Error> {
        let latest_header = reqwest::blocking::get(
            "https://raw.githubusercontent.com/dalance/cargo-trend/master/db_v3/db.json",
        )?
        .text()?;

        let header: DbHeader = serde_json::from_str(&latest_header)?;
        let path = dir.as_ref().join("db.json");
        let mut file = File::create(path)?;
        file.write_all(latest_header.as_bytes())?;
        file.flush()?;

        for (i, h) in header.hash.iter().enumerate() {
            let path = dir.as_ref().join(format!("db{}", i));
            let fetch = if path.exists() {
                let path = dir.as_ref().join(format!("db{}", i));
                let mut file = File::open(&path)?;
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                let hash = format!("{:x}", Sha256::digest(&buf));
                &hash != h
            } else {
                true
            };

            if fetch {
                let mut res = reqwest::blocking::get(format!(
                    "https://github.com/dalance/cargo-trend/raw/master/db_v3/db{}",
                    i
                ))?;
                let mut buf = Vec::new();
                res.read_to_end(&mut buf)?;
                let path = dir.as_ref().join(format!("db{}", i));
                let mut file = File::create(&path)?;
                file.write_all(&buf)?;
                file.flush()?;
            }
        }

        Ok(())
    }
}

fn write_chunk(path: &Path, data: Vec<(String, Entry)>) -> Result<String, Error> {
    let chunk = DbChunk { data };

    let mut encoded = Vec::new();
    let mut serializer = Serializer::new(&mut encoded);
    chunk.serialize(&mut serializer)?;
    let mut file = File::create(&path)?;
    file.write_all(&encoded)?;
    file.flush()?;

    let mut file = File::open(&path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let hash = Sha256::digest(&buf);
    Ok(format!("{:x}", hash))
}

fn gather_transitive(
    id: u32,
    requirement: &VersionReq,
    enabled_features: &[String],
    trace: &mut HashSet<u32>,
    crates_by_id: &[&Crate],
    id_of: &HashMap<&str, u32>,
    cache: &mut [Option<HashSet<u32>>],
) -> (HashSet<u32>, HashSet<u32>) {
    if let Some(cached) = &cache[id as usize] {
        return (cached.clone(), HashSet::new());
    }
    let mut ret_looped = HashSet::new();
    let mut ret_transitive = HashSet::new();
    let c = crates_by_id[id as usize];
    for dep in gather_dependencies(c, requirement, enabled_features) {
        let dep_id = match id_of.get(dep.crate_name()) {
            Some(&i) => i,
            None => continue,
        };
        ret_transitive.insert(dep_id);
        if !trace.contains(&dep_id) {
            let mut dep_features: Vec<_> = dep.features().iter().cloned().collect();
            if dep.has_default_features() {
                dep_features.push(String::from("default"));
            }

            let requirement = match VersionReq::parse(dep.requirement()) {
                Ok(x) => x,
                Err(_) => continue,
            };

            trace.insert(dep_id);
            let (transitive, looped) = gather_transitive(
                dep_id,
                &requirement,
                &dep_features,
                trace,
                crates_by_id,
                id_of,
                cache,
            );
            trace.remove(&dep_id);

            for l in looped {
                ret_looped.insert(l);
            }
            for t in transitive {
                ret_transitive.insert(t);
            }
        } else {
            ret_looped.insert(dep_id);
        }
    }
    cache[id as usize] = Some(ret_transitive.clone());
    (ret_transitive, ret_looped)
}

fn gather_dependencies(
    krate: &Crate,
    requirement: &VersionReq,
    enabled_features: &[String],
) -> Vec<Dependency> {
    let krate = krate
        .versions()
        .iter()
        .filter(|x| {
            if let Ok(version) = Version::parse(x.version()) {
                requirement.matches(&version)
            } else {
                false
            }
        })
        .last();

    let mut ret = Vec::new();
    if let Some(krate) = krate {
        let enabled_deps = gather_enabled_dependencies(
            krate.features(),
            enabled_features,
            100,
            &mut HashSet::new(),
        );

        for dep in krate.dependencies() {
            if dep.is_optional() {
                if enabled_deps.iter().any(|x| x == dep.crate_name()) {
                    ret.push(dep.clone());
                }
            } else {
                ret.push(dep.clone());
            }
        }
    }

    ret
}

fn gather_enabled_dependencies(
    features: &HashMap<String, Vec<String>>,
    enabled_features: &[String],
    max_depth: usize,
    checked: &mut HashSet<String>,
) -> Vec<String> {
    let mut ret = Vec::new();
    for enabled in enabled_features {
        // break feature loop
        if checked.contains(enabled) {
            continue;
        }
        checked.insert(enabled.clone());

        if let Some(expanded) = features.get(enabled) {
            for e in expanded {
                let mut children = if max_depth == 0 {
                    Vec::new()
                } else {
                    gather_enabled_dependencies(features, &vec![e.clone()], max_depth - 1, checked)
                };
                if children.is_empty() {
                    ret.push(e.clone());
                } else {
                    ret.append(&mut children);
                }
            }
        }
    }
    ret
}
