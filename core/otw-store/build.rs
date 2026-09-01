// Build-script hint: `sqlx::migrate!` embeds ./migrations at compile time, and cargo's
// incremental builds don't notice new migration files on their own. Declaring the
// directory here makes cargo rerun this crate's build (and re-expand the macro) whenever
// a migration is added/changed — the stable-Rust fix recommended by SQLx.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
