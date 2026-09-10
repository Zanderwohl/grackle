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

/// The rate physics steps at, and therefore the rate the network ticks at.
///
/// One number rather than two: Lightyear's tick and Bevy's `Time<Fixed>` have
/// to agree exactly, because a tick is the unit a client's prediction is
/// reconciled in. Bevy's default happens to be the same 64 Hz, and it is set
/// from here anyway so that changing it cannot silently change only one half.
pub const TICK_HZ: f64 = 64.0;

/// Refuses connections between builds that do not share it.
///
/// Bump on any change to what goes over the wire. Netcode checks it during the
/// handshake, so a mismatch is a refused connection rather than two processes
/// disagreeing about the meaning of a packet.
pub const NETCODE_PROTOCOL_ID: u64 = 0x6772_6163_6b6c_6501;

/// The shared secret a listen server signs connect tokens with.
///
/// **This is not security.** It is compiled into the binary, so anyone holding
/// a copy of the game can mint a token for any client id. That is the right
/// trade for a LAN listen server with no accounts behind it, and it is exactly
/// what the token backend in `documentation/sketch.md` replaces: a real
/// deployment generates a key per server and hands out tokens from a service
/// that has already authenticated somebody.
pub const DEV_PRIVATE_KEY: [u8; 32] = *b"grackle-development-key-do-not-!";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_text_agrees() {
        assert_eq!(DEFAULT_PORT_TEXT.parse::<u16>(), Ok(DEFAULT_PORT));
    }
}
