mod db;
mod plotter;

use crate::db::Db;
use crate::plotter::Plotter;
use std::path::PathBuf;

fn main() {
    let db_path = PathBuf::from("db/db.gz");
    let mut db = if db_path.exists() {
        Db::load(&db_path).unwrap()
    } else {
        Db::new()
    };
    db.update(None);
    db.save(&db_path);
}
