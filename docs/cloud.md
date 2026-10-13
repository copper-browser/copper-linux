# Copper Cloud: sync with your Mac and other computers

*Settings › Cloud, or Ctrl+K › Copper Cloud…*

Copper links to one self-hosted
[copper-cloud](https://github.com/Exowatt-Labs/copper-cloud) instance at a
time, signs in to an account on it, and syncs the parts of the browser you
switch on. It speaks the same protocol as Copper on the Mac, so a Linux
computer and a Mac signed in to the same account see each other's spaces,
kept tabs, bookmarks, settings and history.

Sync is off until you turn it on. Until then, nothing leaves this computer
except what signing in sends. Shared canvases are the one exception: a shared
canvas lives on the cloud, because that is what sharing means.

## Setting it up

There are three steps, in order. A pairing code from a computer that is
already signed in does all three in one go (see the next section).

1. **Connect.** Paste the link code from your Copper Cloud administrator into
   the one field. If you run the server yourself, `copper-cloud link-code`
   prints one:

   ```text
   copper-cloud://HOST:PORT/#k=<key>&fp=<sha256 of the certificate>
   ```

   The page shows the host and the certificate fingerprint before anything is
   sent. **Connect** checks `/healthz` through the pinned certificate and that
   the instance accepts the key, then keeps the link. *Enter it by hand*
   takes the address, the key and the fingerprint as three fields.

   `k` is either the instance's shared key or a personal access key (`ck_…`)
   the admin made for you. Copper sends it as `X-Copper-Instance` on every
   request. If the key is revoked or expired, the instance refuses this
   Copper; ask the admin for a new link code.
2. **Account.** *Sign in*, or *Create account* with an email, a password of
   at least 10 characters, and the name shown beside your cursor on shared
   canvases.
3. **Sync.** Choose what syncs and press **Turn on sync**. The first sync
   merges this computer with whatever the cloud already has. Nothing on
   either side is thrown away.

After that the page shows:

- the status, with **Sync now** and **Pause…**
- the switches, each of which takes effect at once
- the tabs open on your other devices
- the account, with this device's name
- **Pair another device**
- the instance, with its fingerprint and **Disconnect**
- a log

## Pairing with your Mac

A computer that is already signed in can make a **pairing code**. The code
links another computer to the same instance and signs it in as the same
person, so the new computer only needs one paste.

1. On the computer that is signed in, go to Settings › Cloud › **Pair
   another device** and press **Make a code**. On Linux, Ctrl+K › *Pair
   another device with Copper Cloud* also works; on the Mac the command is
   *Pair another Mac*. The code looks like this:

   ```text
   copper-cloud://HOST:PORT/#p=cp_<32 characters>&fp=<sha256 of the certificate>
   ```

2. On the new computer, paste it into the Connect field in Settings › Cloud.
   The page says *Pairing code — links this device and signs you in*.
3. Press the button. Copper keeps the link, the account and the session
   token in one write, then turns on sync.

A code works **once**, for **10 minutes**. Making a new one revokes the one
shown. A used, revoked or expired code is refused; make a new one.

Pairing works in both directions: a Mac can pair a Linux computer, and a
Linux computer can pair a Mac.

## What syncs

| Switch | What | Notes |
|---|---|---|
| Spaces and kept tabs | Every space (name, colour, icon, theme, profile *name*, order) and its pinned and Saved tabs | Today's tabs stay on this computer. A profile syncs only as a name; the cookies behind it never travel. |
| Settings | A short allowlist (below) | Never passwords, paths, keys, accounts or agent setup. |
| Bookmarks | The whole tree, folders and all | |
| Open tabs | This computer's open (non-pinned) tabs, for your other devices to see | One way only: another device's tabs never open here by themselves. Click one to open it. |
| History | Places you visit | Append-only. Clearing history here, or forgetting a page, deletes it on the cloud too, if the cloud supports that (copper-cloud 0.8.0). |
| Personal canvas | Your Personal canvas | Shown apart, under *Canvas*. Shared canvases ignore every switch. |

**Settings that sync:** `look` (light, dark or system), `sidebar` (tabs in a
column or a row), `glyph`, `tabs.switching`, `tabs.sleep`, `spaces.swipe`,
`shield`, `autocorrect` and `sections.archive`.

**Settings that never sync:** passwords and passkeys, the downloads folder,
the sidebar width, everything in `agent.json`, model keys, Bitwarden and
1Password, and extensions.

**Passwords never sync through Copper Cloud.** On Linux they stay in your
Secret Service keyring, or in Bitwarden or 1Password, which sync on their own.

## How merging works

Spaces, settings and bookmarks are whole documents on the server, each with a
version. A local change waits a moment, then the document goes up with the
version it was based on. If another device wrote first, Copper merges three
ways: the copy last synced, this computer's, and the server's.

- A change on one side only: that side's wins.
- A change on both sides: the server's wins (for settings, key by key).
- Deleted on one side, unchanged on the other: it stays deleted.
- New on either side: it is kept.

History is not a document. New visits go up in batches. Other devices'
visits come down from where the last pull stopped. Copper pulls at sign-in,
at *Turn on sync*, whenever the event stream reconnects, and when another
device writes.

To delete history on the cloud (every device's), run
`copper cloud-history delete --since 24h`, `--host example.com`, or `--all`.
The `cloud_history_delete` MCP tool does the same. History on this computer
is not touched, and other devices keep what they already pulled.

## Where it is kept, and who can read it

| What | Where | Who can read it |
|---|---|---|
| The link and the session token | `cloud.json` in Copper's data folder (`~/.local/share/copper/`, or `~/.var/app/com.copperbrowser.Copper/data/copper/` for the Flatpak), mode 0600 | You |
| Sync state (last-synced copies, history cursors) | `cloud-sync.json` beside it, 0600 | You |
| Model keys the cloud hands out | `cloud-intelligence.json`, 0600, deleted on sign-out | You |
| What you sync | The instance, encrypted at rest with your account's key | You, and that instance's operator |

- **Pinning.** A link code's `fp=` is the SHA-256 of the server's
  certificate. Copper accepts the server only if its certificate has exactly
  that fingerprint, whoever signed it. A code without `fp=` uses the
  system's normal certificate checks.
- **Plain HTTP** is accepted only for a loopback host (a development server
  on `127.0.0.1`).
- **Your cloud password** goes to the server only to sign up or sign in, over
  the pinned connection. It is not kept.
- **Pairing codes** are never written to disk or logged.
- The session token never goes into the system keyring, `settings.json`, the
  log, or any agent or CLI output.

## Troubleshooting

- **"The server's certificate doesn't match the link code's fingerprint."**
  The certificate changed. Get a fresh link code. If the server didn't
  change, something is in the way.
- **"Nothing is listening there" or "No such host."** The address is wrong
  or the instance is down. Run `copper-cloud doctor` on the server.
- **"The instance refused this key."** The code is from another instance, or
  the key was rotated or revoked. Get the link code again.
- **"That pairing code doesn't work any more."** It was used, revoked, or is
  past its ten minutes. Make a new one.
- **The status stays "Can't reach the cloud."** Sync retries on its own once
  the connection is back; **Sync now** tries at once. The log at the foot of
  the page has the details.
- **Something didn't sync.** Check that the switch is on for it. Remember
  what doesn't travel: Today tabs, tab groups, theme pictures, and settings
  outside the allowlist.
