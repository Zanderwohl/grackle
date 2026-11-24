Grackle
=======

Running it
----------

Windows: double-click grackle.exe. A console window opens beside the game and shows its log.

macOS: the build is not signed, so macOS refuses to open it the first time. Either:
  - double-click grackle, dismiss the warning, then open System Settings > Privacy & Security
    and click "Open Anyway" next to the message about grackle; or
  - in Terminal, run this once from the folder holding this file:
        xattr -dr com.apple.quarantine .
Then double-click grackle. It runs in a Terminal window, which shows its log.

Linux: run ./grackle from this folder, or double-click it in a file manager.

The game uses this folder as its working directory wherever it is started from, so keep
grackle, assets/ and the rest together. Move the whole folder anywhere you can write to.

What is in this folder
----------------------

  assets/default/  The shipped pack: maps, props, weapons, materials, translations.
                   A new release replaces it, so do not keep your own work here.
  assets/user/     Made on first launch. Your maps, props, settings and shortcuts go here,
                   and override the default pack. Copy this folder to keep your work when
                   you update.
  assets/<other>/  Any other folder under assets/ is a pack too.
  cache/           Thumbnails, baked lighting and navigation, round records. Safe to delete.
  logs/            One log file per launch.
  downloads/       Files sent by servers you joined.
  screenshots/     F12 saves the window here.

Updating
--------

Unpack the new release somewhere new, then copy assets/user/ (and any packs of your own) from
the old folder into it.
