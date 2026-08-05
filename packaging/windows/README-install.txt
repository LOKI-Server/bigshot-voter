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
  - Start Menu → Big-Shot Voting  (starts server + opens the browser)
  - Or run:  bigshot-voter.exe
  - Or:      launch.cmd

Then open http://127.0.0.1:17843 and register the first account (admin).

Change the data directory
-------------------------
  bigshot-voter.exe --set-data-dir "D:\Servers\Big-Shot"
  bigshot-voter.exe --clear-data-dir

Other flags: --bind, --data-dir (process-only), --max-upload.
See the project README for details.

Uninstall
---------
  Removes program files only. Your data\ folder and AppData config stay
  unless you delete them yourself.
