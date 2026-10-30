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
already signed in does all three in one go (see the next section), and so
does a sign-in link from an instance that offers company sign-in (see
*Signing in with your company account*).

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

### From the command line, with no window

`copper cloud pair` does the same with Copper **not** running, for a headless
Copper (an agent's own browser) or a computer you reach over SSH:

```sh
copper cloud pair 'copper-cloud://HOST:PORT/#p=cp_…&fp=…' --sync tabs --name "Build box"
copper cloud status
```

It links, signs in, adopts the device id the server answers with, and writes
`cloud.json` and the sync switches for the current profile (`COPPER_WORLD`),
so Copper syncs as soon as it starts. `--sync` takes `none`, `all`, or a list
of `spaces,settings,bookmarks,tabs,history,canvas`; without it, the switches
this profile had before, else all of them. `--json` prints one object:
`{"ok":true,"email","device_id","device_name","host","sync":[…],"provided":[…]}`,
or `{"ok":false,"error","message"}`. Neither form ever prints the session
token or a key.

It refuses while a Copper window has the profile open (that window keeps its
own copy of `cloud.json` and would write over the pairing): quit Copper
first, or pair in Settings › Cloud. Exit codes: 0 done, 1 the cloud refused or
couldn't be reached, 2 a usage error or a running Copper.

A deep link `copper://cloud/join?code=<the code, percent-encoded>` carries
any of these codes. Copper registers itself for `copper://` links, and since
0.4.1 such a link opens **Settings › Cloud** with the code filled in —
clicked on a web page (the instance's *Open in Copper*), run as
`copper 'copper://cloud/join?code=…'` (a running Copper takes it over), or
typed into the address field, which offers *Join Copper Cloud — HOST*
instead of a search. It never opens as a tab. A sign-in link (below) starts
the sign-in at once; any other code waits for you to press Connect or Pair.

A link never switches Copper to another instance by itself. Connected to
another instance already, Settings › Cloud says *This link is for HOST*,
with **Dismiss**, and **Disconnect and use it…** (which asks first). Signed
in to that same instance already, it says *Already signed in to HOST as
EMAIL*. The command line reads deep links too
(`copper cloud pair 'copper://cloud/join?code=…'`).

## Signing in with your company account

An instance can let people sign in with their company account (Google
Workspace, say) instead of a password. Its admin hands out a **sign-in link**
instead of a link code:

```text
copper-cloud://HOST:PORT/#sso=google
copper-cloud://HOST:PORT/#sso=google&fp=<sha256 of the certificate>
```

It has no key in it: anyone may have it. Signing in is what proves who you
are, and the instance makes the account the first time (if its admin lets
it).

**In Settings › Cloud** (0.4.1), paste the sign-in link where a link code
goes. Copper shows the host and the fingerprint, if the link has one, and a
**Continue with Google** button (the name is the instance's own for its
provider). Press it and your browser opens; the row becomes *Finish signing
in in your browser…* with a spinner and **Cancel**. When the browser comes
back you are signed in, and sync turns on with every switch (or the ones
this device had before). A self-signed instance with no `fp=` in the link
gets the same step as a pairing code: Copper shows its certificate's
fingerprint to compare, and **Trust and continue** pins it.

An instance you connected to with a link code shows the same **Continue
with Google** button above the email and password form, when it offers
single sign-on, with the domains it takes (*Use your @example.com account*).
If its admin has turned password sign-in off for everyone but admins, the
form waits behind **Sign in with a password instead**.

**From the command line** (no window needed):

```sh
copper cloud sso 'copper-cloud://HOST:PORT/#sso=google'
copper cloud sso            # the instance this Copper is connected to already
```

Copper opens your browser at the company's sign-in page and waits up to
**5 minutes** (Cancel, or Ctrl+C on the command line, gives up). It listens for the answer on a loopback port
of its own (`127.0.0.1`, a fresh port each time), checks that the answer is
for the request it made, and finishes the sign-in with the instance using a
PKCE code verifier that never left this computer. The browser tab then says
*You're signed in — return to Copper*. If the browser doesn't open, the
address is printed; open it yourself on this computer.

After that it is exactly as if you had paired: the link, the account and the
session token in one write, the device id the server chose, and sync on —
in Settings › Cloud through the running window's own sync, on the command
line with the same `--sync` choice as `copper cloud pair`.

Copper asks the instance which sign-ins it offers (`GET /v1/auth/sso`); an
instance from before single sign-on answers 404, and Copper says so.

## What syncs

| Switch | What | Notes |
|---|---|---|
| Spaces and kept tabs | Every space (name, colour, icon, theme, profile *name*, order) and its pinned and Saved tabs | Today's tabs stay on this computer. A profile syncs only as a name; the cookies behind it never travel. Closing a Saved tab (Ctrl+W, an agent, `copper close`) only puts it to sleep, so it stays kept everywhere; the × on its row, the menu's Close Tab or Unsave remove it, and that removal is what syncs. |
| Settings | A short allowlist (below) | Never passwords, paths, keys, accounts or agent setup. |
| Bookmarks | The whole tree, folders and all | |
| Open tabs | This computer's open (non-pinned) tabs, for your other devices to see | One way only: another device's tabs never open here by themselves. Click one to open it. |
| History | Places you visit | Append-only. Clearing history here, or forgetting a page, deletes it on the cloud too, if the cloud supports that (copper-cloud 0.8.0). |
| Personal canvas | Your Personal canvas | Shown apart, under *Canvas*. Shared canvases ignore every switch. |

**Settings that sync:** `look` (light, dark or system), `sidebar` (tabs in a
column or a row), `glyph`, `tabs.switching`, `tabs.sleep`, `spaces.swipe`,
`shield`, `autocorrect` and `sections.archive`.

**Settings that never sync:** passwords and passkeys, the downloads folder,
the sidebar width, the zoom kept for each site (`zoom.<host>`), everything in `agent.json`, model keys, Bitwarden and
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

Spaces add one rule: **a computer only overwrites what was changed on it.**
Copper notes each change you make here to kept tabs and folders — filing a
tab, taking it out of a folder, saving, unsaving or removing a kept tab,
deleting a folder, whether by hand, by dragging or through an agent's
tools. For anything the server already has, only those changes go up;
anything else this computer has (a session copied from another computer,
folders it never got) is taken back to the server's copy instead of
published. The notes are kept in `cloud-sync.json`, so a change made
offline still goes up after a relaunch, and each is dropped once the server
has it. The first spaces sync after launch, and after every reconnect,
takes the server's copy first.

If one sync would unfile or remove more than 10 kept tabs, or delete a
folder that still holds tabs, Copper stops and asks:

- *This computer is about to unfile 12 tabs on all your devices* —
  **Sync It** sends it; **Undo Here** puts this computer back the way your
  other devices have it. Deleting a space you confirmed is not asked again.
- *Another device unfiled 12 tabs* — **Accept** takes it here; **Keep
  Mine** keeps your folders and tabs and sends them back to the others.

Every spaces publish and apply is logged (`cloud publish|apply doc=spaces
v=… unfiled=N removed=M folders-=K held=yes|no`), in the log and in the
cloud's own log.

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
  that fingerprint, whoever signed it. A code without `fp=` (a server with a
  certificate from a public authority, such as Let's Encrypt) is checked
  against the public certificate authorities Copper has built in (Mozilla's
  list), with the host name. A self-signed server without `fp=` is refused,
  and Copper shows its fingerprint so you can compare it and pin it on
  purpose (`--trust FP` on the command line).
- **Plain HTTP** is accepted only for a loopback host (a development server
  on `127.0.0.1`).
- **Your cloud password** goes to the server only to sign up or sign in, over
  the pinned connection. It is not kept.
- **Pairing codes** are never written to disk or logged. Nor are the
  one-time codes and verifiers of a company sign-in; the loopback port that
  waits for it accepts only this computer, and only the request it made.
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
- **"That's a sign-in link for HOST."** It was given to `copper cloud pair`
  (or an older Copper). Paste it in Settings › Cloud, or run
  `copper cloud sso` with it.
- **"This link is for HOST."** Copper is connected to another instance.
  Dismiss it, or Disconnect and use it (nothing is deleted).
- **"Use your @example.com account."** The instance takes accounts only from
  that domain: sign in again and pick your company account.
- **"This instance doesn't offer single sign-on."** It is older than
  copper-cloud's single sign-on, or its admin turned it off. Use a link code
  and a password, or a pairing code.
- **"Nothing came back from the browser in 5 minutes — try again."** Press
  Continue again (or run `copper cloud sso` again), and finish in the browser
  it opens.
- **"Copper is running on this profile."** `copper cloud pair` and
  `copper cloud sso` need the profile to themselves. Quit Copper, or use
  Settings › Cloud.
- **The status stays "Can't reach the cloud."** Sync retries on its own once
  the connection is back; **Sync now** tries at once. The log at the foot of
  the page has the details.
- **Something didn't sync.** Check that the switch is on for it. Remember
  what doesn't travel: Today tabs, tab groups, theme pictures, and settings
  outside the allowlist.
