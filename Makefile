build_db_v2:
	rm -rf ./db_v2
	./target/release/cargo-trend trend -u ./db_v2/db.gz -b snapshot-2018-09-26
	./target/release/cargo-trend trend -u ./db_v2/db.gz -b snapshot-2019-10-17
	./target/release/cargo-trend trend -u ./db_v2/db.gz
