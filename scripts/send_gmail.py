#!/usr/bin/env python3
"""Send an e-mail via the Gmail API (OAuth) — reference helper for BigTranscriber.

BigTranscriber's auto-e-mail feature shells out to a small sender script instead
of embedding SMTP credentials. This is that script. It uses the Gmail API with an
OAuth token you provide, so mail is genuinely sent *From* your Gmail address.

Setup (once):
  1. Create an OAuth client (Desktop) in Google Cloud, enable the Gmail API.
  2. Authorize it for the scope  https://www.googleapis.com/auth/gmail.send
     and save the resulting token JSON somewhere private.
  3. pip install google-api-python-client google-auth
  4. Point the app at this script + your token via env vars:
        BIGTRANSCRIBER_GMAIL_PY=/path/to/send_gmail.py
        BIGTRANSCRIBER_GMAIL_PYTHON=/path/to/python
        GMAIL_TOKEN=/path/to/token.json
        GMAIL_FROM=you@gmail.com        # optional; defaults to the token's account

Usage:
  send_gmail.py --to a@b.com --subject "Hi" --body-file body.txt \
      [--html] [--reply-to r@x.com] [--cc c@x.com] [--attach f ...] [--dry-run]
"""
import argparse
import base64
import mimetypes
import os
import sys
from email import encoders
from email.mime.base import MIMEBase
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText

from google.oauth2.credentials import Credentials
from google.auth.transport.requests import Request
from googleapiclient.discovery import build

TOKEN = os.environ.get("GMAIL_TOKEN", os.path.expanduser("~/.config/bigtranscriber/gmail_token.json"))
FROM = os.environ.get("GMAIL_FROM", "me")


def gmail_client():
    creds = Credentials.from_authorized_user_file(TOKEN)
    if not creds.valid and creds.refresh_token:
        creds.refresh(Request())
    return build("gmail", "v1", credentials=creds)


def build_message(args) -> dict:
    body = open(args.body_file, encoding="utf-8").read() if args.body_file else (args.body or "")
    if args.attach:
        msg = MIMEMultipart()
        msg.attach(MIMEText(body, "html" if args.html else "plain", "utf-8"))
        for path in args.attach:
            ctype, _ = mimetypes.guess_type(path)
            maintype, subtype = (ctype or "application/octet-stream").split("/", 1)
            part = MIMEBase(maintype, subtype)
            with open(path, "rb") as f:
                part.set_payload(f.read())
            encoders.encode_base64(part)
            part.add_header("Content-Disposition", "attachment", filename=os.path.basename(path))
            msg.attach(part)
    else:
        msg = MIMEText(body, "html" if args.html else "plain", "utf-8")
    if FROM and FROM != "me":
        msg["From"] = FROM
    msg["To"] = ", ".join(args.to)
    if args.cc:
        msg["Cc"] = ", ".join(args.cc)
    if args.bcc:
        msg["Bcc"] = ", ".join(args.bcc)
    if args.reply_to:
        msg["Reply-To"] = args.reply_to
    msg["Subject"] = args.subject
    return {"raw": base64.urlsafe_b64encode(msg.as_bytes()).decode()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--to", action="append", required=True)
    ap.add_argument("--cc", action="append")
    ap.add_argument("--bcc", action="append")
    ap.add_argument("--subject", required=True)
    ap.add_argument("--body")
    ap.add_argument("--body-file")
    ap.add_argument("--html", action="store_true")
    ap.add_argument("--attach", action="append")
    ap.add_argument("--reply-to")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    if not args.body and not args.body_file:
        sys.exit("need --body or --body-file")
    payload = build_message(args)
    if args.dry_run:
        print(f"[dry-run] To {args.to} Subj {args.subject!r} "
              f"({'html' if args.html else 'text'}, {len(args.attach or [])} attachments)")
        return
    sent = gmail_client().users().messages().send(userId="me", body=payload).execute()
    print(f"[ok] sent via Gmail API, id={sent.get('id')} to {args.to}")


if __name__ == "__main__":
    main()
