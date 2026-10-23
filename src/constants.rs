pub const MAP_BLUEPRINT_EXTENSION: &str = "gmb";
/// Where maps live inside a pack.
pub const BLUEPRINTS_DIR: &str = "blueprints";
pub const MAP_BACKUP_EXTENSION: &str = "bak";

/// What `--serve` binds and a bare hostname connects to. Unregistered, and
/// not a Source engine port, so both can run on one machine.
pub const DEFAULT_PORT: u16 = 27100;

/// `DEFAULT_PORT` as text, since clap wants a `&'static str` default.
pub const DEFAULT_PORT_TEXT: &str = "27100";

/// Physics rate and network tick. Lightyear's tick and `Time<Fixed>` must
/// agree exactly, since prediction reconciles in ticks.
pub const TICK_HZ: f64 = 64.0;

/// One tick of [`TICK_HZ`].
pub fn one_tick() -> std::time::Duration {
    std::time::Duration::from_secs_f64(1.0 / TICK_HZ)
}

/// Bump on any change to what goes over the wire; netcode refuses a handshake
/// between builds that differ.
pub const NETCODE_PROTOCOL_ID: u64 = 0x6772_6163_6b6c_6501;

/// Signs connect tokens. Not security: it is in the binary, so anyone can mint
/// a token. Replaced by the token backend in `documentation/sketch.md`.
pub const DEV_PRIVATE_KEY: [u8; 32] = *b"grackle-development-key-do-not-!";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_text_agrees() {
        assert_eq!(DEFAULT_PORT_TEXT.parse::<u16>(), Ok(DEFAULT_PORT));
    }
}
