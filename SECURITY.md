# Security policy

git-evidence runs `git` as your user, on repositories you point it at. It starts no server and opens no network connection of its own; `fetch` and `push` go through Git and your configured remotes.

## Pushed evidence is permanent

Evidence pushed to a remote is in every clone of it, and removing a note does not remove it from history. Before anything is sent, `push` runs the same audit as `git evidence audit` over every note version the remote lacks, and refuses while there is a finding: a run that does not fold, or text that looks personal (this machine's host names, the user's login and home directory, filesystem paths, addresses). Pushing is also off until `git config --local evidence.push true`. Run `git evidence audit` and `git evidence push --dry-run` before a first push.

The audit is a safety net, not a guarantee: it cannot recognise every secret. What a producer records (parameters, descriptors, close conditions, keys, reasons, `source`) is published as written. Do not record credentials, tokens or private data in evidence.

## If private data was pushed

1. Stop pushing from every store that has it.
2. Rewrite the affected `refs/notes/evidence/<kind>` history to drop the lines, in one store, and force-update the remote's ref. Every other store must then discard its copy (`git update-ref -d refs/notes/remotes/<remote>/evidence/<kind>` and the local ref) and fetch again; otherwise its next `fetch` and `push` restore the lines by union merge.
3. Treat the data as disclosed: rotate any credential it contained.

## Reporting a vulnerability

Please report a vulnerability privately, through GitHub's private vulnerability reporting: on the repository's **Security** tab, choose **Report a vulnerability**. Do not open a public issue for it. This includes a way to make the audit pass on personal text it is meant to catch, or to make `git evidence` act on another repository or configuration than the one it was given.

Include the version (`git evidence --version`), the Git version, what you ran, and what an attacker could do. You will get a reply within a week. A fix is released before the details are made public.

## Supported versions

git-evidence is pre-1.0: only the latest release receives fixes.
