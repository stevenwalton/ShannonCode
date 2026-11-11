# Security

Shannon decides which commands and file writes an AI model may run on your
machine, and keeps the values you mark secret out of what the model, the
session store and the transcript see. A way around either is a security
problem, and it is wanted.

## Reporting

Report it privately, through GitHub's private vulnerability reporting: the
repository's **Security** tab, then **Report a vulnerability**. Please do not
open a public issue for it. Say what you ran, what Shannon decided or showed,
and what you expected; a command line that reproduces it is the most useful
thing you can send.

What counts, for example:

- a command or a file write that runs unasked where your rules say it should
  ask or be refused;
- a value you marked secret reaching the model, the session store, the
  transcript or a log;
- Shannon reading or writing outside the places you approved;
- anything a directory you open can do to your machine before you trust it.

## Supported versions

Shannon is built from source, and only the latest `main` is supported: fixes
land there.

## Past reports

`docs/Security/` holds what was found and fixed during development, each
report with its severity, its scope, and how to check the fix.
