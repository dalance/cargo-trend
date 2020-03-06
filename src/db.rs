use anyhow::Error;
use chrono::serde::ts_seconds;
use chrono::{DateTime, TimeZone, Utc};
use crates_index::{Crate, Dependency, Index};
use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};
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
            let master = repo.find_branch(&format!("origin/{}", branch), BranchType::Remote)?;
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

        let total = revs.len();
        for (i, (time, id)) in revs.iter().enumerate() {
            println!("Update DB: {} {} ( {} / {} )", time, id, i + 1, total);
            let obj = repo.find_object(id.clone(), None)?;
            repo.reset(&obj, ResetType::Hard, None)?;

            let index = Index::new(&dir.path());
            let mut crates = HashMap::new();
            for c in index.crates() {
                crates.insert(String::from(c.name()), c);
            }

            let mut deps = HashMap::new();
            let mut cache = HashMap::new();
            for (name, c) in &crates {
                let enabled_features = [String::from("default")];

                for dep in gather_dependencies(c, &VersionReq::any(), &enabled_features) {
                    let name = dep.crate_name();
                    if let Some((cnt, _)) = deps.get_mut(name) {
                        *cnt += 1;
                    } else {
                        deps.insert(String::from(name), (1, 0));
                    }
                }

                let mut trace = HashSet::new();
                trace.insert(String::from(name));
                let (transitive, looped) = gather_transitive(
                    name,
                    &VersionReq::any(),
                    &enabled_features,
                    trace,
                    &crates,
                    &mut cache,
                );

                // connect looped transitive
                for l in &looped {
                    let l_cached = cache.get(l).unwrap().clone();

                    for t in &transitive {
                        let t_cached = cache.get_mut(t).unwrap();
                        if t_cached.contains(l) {
                            for l in &l_cached {
                                t_cached.insert(l.clone());
                            }
                        }
                    }
                }

                for t in &transitive {
                    if let Some((_, cnt)) = deps.get_mut(t) {
                        *cnt += 1;
                    } else {
                        deps.insert(String::from(t), (0, 1));
                    }
                }
            }

            let total_crates = index.crates().count() as u64;
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
}

fn gather_transitive(
    name: &str,
    requirement: &VersionReq,
    enabled_features: &[String],
    trace: HashSet<String>,
    crates: &HashMap<String, Crate>,
    cache: &mut HashMap<String, HashSet<String>>,
) -> (HashSet<String>, HashSet<String>) {
    if let Some(cached) = cache.get(name) {
        (cached.clone(), HashSet::new())
    } else {
        let mut ret_looped = HashSet::new();
        let mut ret_transitive = HashSet::new();
        if let Some(c) = crates.get(name) {
            for dep in gather_dependencies(c, requirement, enabled_features) {
                let name = dep.crate_name();
                ret_transitive.insert(String::from(name));
                if !trace.contains(name) {
                    let mut trace = trace.clone();
                    trace.insert(String::from(name));

                    let mut dep_features: Vec<_> =
                        dep.features().iter().map(|x| x.clone()).collect();
                    if dep.has_default_features() {
                        dep_features.push(String::from("default"));
                    }

                    let requirement = VersionReq::parse(dep.requirement()).unwrap();
                    let (transitive, looped) =
                        gather_transitive(name, &requirement, &dep_features, trace, crates, cache);
                    for l in looped {
                        ret_looped.insert(l.clone());
                    }
                    for t in transitive {
                        ret_transitive.insert(t.clone());
                    }
                } else {
                    ret_looped.insert(String::from(name));
                }
            }
        }
        cache.insert(String::from(name), ret_transitive.clone());
        (ret_transitive, ret_looped)
    }
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
            let version = Version::parse(x.version()).unwrap();
            requirement.matches(&version)
        })
        .last();

    let mut ret = Vec::new();
    if let Some(krate) = krate {
        let enabled_deps = gather_enabled_dependencies(krate.features(), enabled_features, 100);

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
) -> Vec<String> {
    let mut ret = Vec::new();
    for enabled in enabled_features {
        if let Some(expanded) = features.get(enabled) {
            for e in expanded {
                let mut children = if max_depth == 0 {
                    Vec::new()
                } else {
                    gather_enabled_dependencies(features, &vec![e.clone()], max_depth - 1)
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
