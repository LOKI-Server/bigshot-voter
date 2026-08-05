Big-Shot Voting — Windows install
=================================

Where things live
-----------------
  Program files:  %LOCALAPPDATA%\Programs\bigshot-voter\
  Runtime data:   %LOCALAPPDATA%\Programs\bigshot-voter\data\
                  (database, cookie secret, uploads — next to the .exe)
  Config:         %APPDATA%\bigshot-voter\bigshot.json

The installer defaults to a per-user folder under Local AppData so the
process can create and write the data directory without administrator
rights. You do not need --set-data-dir for a normal install.

Start the server
----------------
  - Start Menu → Big-Shot Voting
  - Or run:  bigshot-voter.exe

The server starts in the system tray (notification area). There is no
console window by default.

  Right-click the tray icon → Show Console  (server log window)
                           → Exit
  Minimize the console to hide it again; close the console to quit.
  Left-click the tray icon also shows the console.

Then open http://127.0.0.1:17843 and register the first account (admin).

Change the data directory
-------------------------
  bigshot-voter.exe --set-data-dir "D:\Servers\Big-Shot"
  bigshot-voter.exe --clear-data-dir

Other flags: --bind, --data-dir (process-only), --max-upload,
  --foreground (keep console open), --no-tray.
See the project README for details.

Uninstall
---------
  Removes program files only. Your data\ folder and AppData config stay
  unless you delete them yourself.
