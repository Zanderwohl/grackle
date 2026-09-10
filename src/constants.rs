pub const SCHEMA_VERSION: u64 = 3;
pub const MAP_BLUEPRINT_EXTENSION: &str = "gmb";
pub const MAP_BACKUP_EXTENSION: &str = "bak";

/// The port `--serve` binds and the connect dialog offers when the user types
/// a bare hostname. Unregistered, and deliberately not one of the Source
/// engine's: a machine running both should not have to think about it.
pub const DEFAULT_PORT: u16 = 27100;

/// The same number as text, because clap wants a `&'static str` for a default
/// and formatting one at startup would have to be leaked to get there.
/// `the_default_port_text_agrees` keeps the two honest.
pub const DEFAULT_PORT_TEXT: &str = "27100";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_text_agrees() {
        assert_eq!(DEFAULT_PORT_TEXT.parse::<u16>(), Ok(DEFAULT_PORT));
    }
}
