#!/usr/bin/env python3
"""Mirror built drawing sets (.pptx) to the project's Google Drive folder (googleslides.md).

For each diagrams/out/<name>.pptx this keeps two reference copies in Drive:
  <folder>/<name>             Google Slides (converted; the copy people open and share)
  <folder>/pptx/<name>.pptx   the exact .pptx that was built (binary, updated in place)

File IDs and links are recorded in diagrams/drive-index.json (commit it), so later syncs
update the same files and links in documents stay valid.

Setup (once):
  pip install google-api-python-client google-auth-oauthlib
  Put an OAuth "Desktop app" client file at ~/.config/eng-slides/credentials.json
  (Google Cloud console: enable the Drive API, create OAuth client ID, type Desktop app).
  First run opens a browser to authorize; the token is cached next to credentials.json.

Usage:
  python diagrams/tools/sync_drive.py diagrams/out/<set>.pptx [more.pptx ...]
  python diagrams/tools/sync_drive.py --all
"""
import argparse
import datetime
import json
import sys
from pathlib import Path

from google.auth.transport.requests import Request
from google.oauth2.credentials import Credentials
from google_auth_oauthlib.flow import InstalledAppFlow
from googleapiclient.discovery import build
from googleapiclient.errors import HttpError
from googleapiclient.http import MediaFileUpload

ROOT = Path(__file__).resolve().parents[1]           # diagrams/
INDEX = ROOT / "drive-index.json"
CONFIG = ROOT / "drive-config.json"                   # {"folder_id": "..."}
CRED_DIR = Path.home() / ".config" / "eng-slides"
SCOPES = ["https://www.googleapis.com/auth/drive"]
PPTX = "application/vnd.openxmlformats-officedocument.presentationml.presentation"
SLIDES = "application/vnd.google-apps.presentation"
FOLDER = "application/vnd.google-apps.folder"
FIELDS = "id,name,webViewLink,modifiedTime"


def service():
    token = CRED_DIR / "token.json"
    creds = Credentials.from_authorized_user_file(token, SCOPES) if token.exists() else None
    if not creds or not creds.valid:
        if creds and creds.expired and creds.refresh_token:
            creds.refresh(Request())
        else:
            flow = InstalledAppFlow.from_client_secrets_file(CRED_DIR / "credentials.json", SCOPES)
            creds = flow.run_local_server(port=0)
        token.write_text(creds.to_json())
    return build("drive", "v3", credentials=creds)


def find(drive, name, parent, mime):
    q = (f"name = '{name.replace(chr(39), chr(92) + chr(39))}' and '{parent}' in parents "
         f"and mimeType = '{mime}' and trashed = false")
    r = drive.files().list(q=q, fields=f"files({FIELDS})", supportsAllDrives=True,
                           includeItemsFromAllDrives=True).execute()
    return r["files"][0] if r["files"] else None


def exists(drive, file_id):
    try:
        f = drive.files().get(fileId=file_id, fields="id,trashed", supportsAllDrives=True).execute()
        return not f.get("trashed")
    except HttpError:
        return False


def subfolder(drive, parent, name):
    f = find(drive, name, parent, FOLDER)
    if f:
        return f["id"]
    body = {"name": name, "mimeType": FOLDER, "parents": [parent]}
    return drive.files().create(body=body, fields="id", supportsAllDrives=True).execute()["id"]


def sync_one(drive, pptx, folder_id, index):
    name = pptx.stem
    entry = index.get(name, {})
    media = lambda: MediaFileUpload(str(pptx), mimetype=PPTX, resumable=True)

    # 1. Exact .pptx, updated in place.
    pptx_folder = subfolder(drive, folder_id, "pptx")
    pid = entry.get("pptx_id")
    if not (pid and exists(drive, pid)):
        hit = find(drive, pptx.name, pptx_folder, PPTX)
        pid = hit["id"] if hit else None
    if pid:
        p = drive.files().update(fileId=pid, media_body=media(), fields=FIELDS, supportsAllDrives=True).execute()
    else:
        body = {"name": pptx.name, "parents": [pptx_folder]}
        p = drive.files().create(body=body, media_body=media(), fields=FIELDS, supportsAllDrives=True).execute()

    # 2. Google Slides copy. Try to re-import into the existing file so its link stays the same.
    sid = entry.get("slides_id")
    if not (sid and exists(drive, sid)):
        hit = find(drive, name, folder_id, SLIDES)
        sid = hit["id"] if hit else None
    s, note = None, ""
    if sid:
        try:
            s = drive.files().update(fileId=sid, media_body=media(), fields=FIELDS, supportsAllDrives=True).execute()
        except HttpError as e:
            # In-place re-import refused: keep the old copy under a dated name, create a new one.
            stamp = datetime.date.today().strftime("%m-%d-%Y")
            drive.files().update(fileId=sid, body={"name": f"{name} (superseded {stamp})"},
                                 supportsAllDrives=True).execute()
            note = f"in-place update refused ({e.resp.status}); previous copy renamed, new link issued"
    if s is None:
        body = {"name": name, "mimeType": SLIDES, "parents": [folder_id]}
        s = drive.files().create(body=body, media_body=media(), fields=FIELDS, supportsAllDrives=True).execute()

    index[name] = {
        "slides_id": s["id"], "slides_link": s["webViewLink"],
        "pptx_id": p["id"], "pptx_link": p["webViewLink"],
        "synced": datetime.datetime.now().isoformat(timespec="seconds"),
    }
    print(f"{name}: {s['webViewLink']}" + (f"  [{note}]" if note else ""))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("files", nargs="*", type=Path)
    ap.add_argument("--all", action="store_true", help="sync every diagrams/out/*.pptx")
    a = ap.parse_args()
    files = sorted((ROOT / "out").glob("*.pptx")) if a.all else a.files
    if not files:
        ap.error("no .pptx files given")
    folder_id = json.loads(CONFIG.read_text())["folder_id"]
    index = json.loads(INDEX.read_text()) if INDEX.exists() else {}
    drive = service()
    for f in files:
        sync_one(drive, f.resolve(), folder_id, index)
    INDEX.write_text(json.dumps(index, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
