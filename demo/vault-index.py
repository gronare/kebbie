#!/usr/bin/env python3
# Writes the demo vault's index as notes-vault-mcp would, so the Areas page has system notes, notes and logs to read: vault-index.py <index file> <home>.
import sqlite3
import sys
import time

index, home = sys.argv[1], sys.argv[2]
db = sqlite3.connect(index)
db.executescript("""
CREATE TABLE notes (key TEXT PRIMARY KEY, version TEXT, mtime REAL, folder TEXT, stem TEXT, title TEXT, summary TEXT, status TEXT,
  kind TEXT, area TEXT, tags TEXT, paths TEXT, date TEXT, updated TEXT, superseded_by TEXT, valid INT, error TEXT, size INT, priority TEXT, source TEXT);
CREATE TABLE repos (repo TEXT PRIMARY KEY, root TEXT, seen_at TEXT);
CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT);
CREATE VIRTUAL TABLE notes_fts USING fts5(title, summary, tags, body, stem, stemmed, key UNINDEXED);
""")
now = time.time()
today = time.strftime("%Y-%m-%d")


def note(key, title, status, kind, area=None, paths=None, priority=None, summary=""):
    folder, stem = key.split("/")[0], key.split("/")[-1][:-3]
    db.execute("INSERT INTO notes (key, mtime, folder, stem, title, summary, status, kind, area, paths, date, updated, valid, priority) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
               (key, now, folder, stem, title, summary, status, kind, area and f"[[{area}]]", paths and f'["{paths}"]', today, today, priority))


def log(key, body):
    db.execute("INSERT INTO notes_fts (key, body) VALUES (?, ?)", (key, body))


note("Areas/shop.md", "shop", "active", "system", paths="~/code/shop", summary="The storefront: product pages, search, cart and checkout.")
note("Areas/api.md", "api", "active", "system", paths="~/code/api", summary="Orders, invoices and API keys for the shop.")
note("Areas/storybook.md", "storybook", "active", "system", paths="~/code/storybook", summary="The shop's component library.")
note("Projects/shop-gift-cards.md", "Gift cards", "backlog", "task", area="shop", priority="medium")
note("Projects/shop-wishlist.md", "Wishlist", "backlog", "task", area="shop", priority="high")
note("Projects/api-order-export.md", "Order export", "backlog", "task", area="api", priority="medium")
note("Projects/api-slow-query-log.md", "Slow query log", "backlog", "task", area="api", priority="low")
note("Projects/storybook-dark-mode.md", "Dark mode for every component", "backlog", "task", area="storybook", priority="low")
log("Log/shop-log.md", "- [2026-10-17] Search products by name | commits: 4f1c2aa | [[shop]]\n- [2026-10-18] Checkout copy added | commits: 9b0e7d1 | [[shop]]\n- [2026-10-19] A cart kept in memory for the session | commits: 5c3a9e0 | [[shop]]")
log("Log/api-log.md", "- [2026-10-16] VAT on invoices | commits: 0d8f3b2 | [[api]]\n- [2026-10-18] Every request logged with its key | commits: 7e21c4f | [[api]]")
log("Log/storybook-log.md", "- [2026-09-30] Buttons and inputs on the new tokens | commits: a17b0c9 | [[storybook]]")
db.execute("INSERT INTO repos VALUES ('shop', ?, ?)", (f"{home}/code/shop", today))
db.execute("INSERT INTO repos VALUES ('api', ?, ?)", (f"{home}/code/api", today))
db.execute("INSERT INTO meta VALUES ('last_sync', ?)", (str(now),))
db.commit()
