mod db;
mod plotter;

use crate::db::Db;
use crate::plotter::Plotter;
use std::path::PathBuf;

fn main() {
    //let db_path = PathBuf::from("db/db.gz");
    //let mut db = if db_path.exists() {
    //    Db::load(&db_path).unwrap()
    //} else {
    //    Db::new()
    //};
    //db.update(None);
    //db.save(&db_path);

    let db_path = PathBuf::from("db/db.gz");

    let db = if db_path.exists() {
        Db::load(db_path).unwrap()
    } else {
        Db::new()
    };

    let targets = vec![
        "failure",
        "error-chain",
        "quick-error",
        "snafu",
        "err-derive",
    ];

    //let targets = vec!["clap", "structopt", "docopt", "argparse", "getopts"];

    //let targets = vec![
    //    "ansi_term",
    //    "termcolor",
    //    "term",
    //    "termion",
    //    "colored",
    //    "console",
    //];

    let plotter = Plotter::new().size((1200, 800));
    let _ = plotter.plot("trend.svg", &targets, &db);
}
