macro_rules! crate_name {
    () => {
        env!("CARGO_CRATE_NAME")
    }
}

#[cfg(debug_assertions)]
const DEFAULT_FILTER: &'static str = concat!(crate_name!(), "=debug");
#[cfg(not(debug_assertions))]
const DEFAULT_FILTER: &'static str = concat!(crate_name!(), "=info");

pub fn init() {
    use env_logger::Env;
    let cfg = Env::default()
        .default_filter_or(DEFAULT_FILTER);
    env_logger::Builder::from_env(cfg)
        .init();
}
