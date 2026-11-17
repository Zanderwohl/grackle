# Notifications and toasts

There is no notification or toast system yet. This is a running list of events that should
raise one when it exists. Until then, each event either writes a line to the log or reports
nothing. Add to the list when you write code that tells the user something only through the log.

The system has to work in Play and in every authoring mode, and it should be plain Bevy UI or
something else that reaches a build with no editor, as the pause menu does. Some events below
happen mid-match.

| Event | Who sees it | Now | Where |
| --- | --- | --- | --- |
| A screenshot was saved, with its file name | The person who pressed the key | `info!` line | [`screenshot.rs`](../src/common/screenshot.rs) |
| A screenshot could not be saved, with the reason | The person who pressed the key | `warn!` line | [`screenshot.rs`](../src/common/screenshot.rs) |
| Somebody joined, with their name | Host | Nothing; the Multiplayer menu shows a count of clients | [`net_transport.rs`](../src/common/net_transport.rs) `report_status` |
| Somebody left, with their name | Host | Nothing | [`net_transport.rs`](../src/common/net_transport.rs) `report_status` |
| Joined a server, with its address | Client | `NetStatus::Connected`, a line inside the Multiplayer menu | [`net_transport.rs`](../src/common/net_transport.rs) `report_status` |
| Left a server (Disconnect) | Client | Nothing | [`pause_menu.rs`](../src/game/pause_menu.rs), [`net_menu.rs`](../src/editor/net_menu.rs) |
| The server went away, or a connection attempt timed out, with the reason | Client | `NetStatus::Failed` in the Multiplayer menu; `drop_a_failed_link` goes solo | [`net_transport.rs`](../src/common/net_transport.rs) `drop_a_failed_link` |

Other players joining and leaving a server should probably reach clients too. That needs a
message from the server, since a client sees only bodies appear and disappear.
