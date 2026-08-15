# GBSelfTest (vendored)

A Game Boy cartridge that tests the machine it is running on and prints a
report over the link port, with a stable code per check and a URL per failure.

* **Upstream:** <https://github.com/Alchemy86/GBSelfTest>
* **Licence:** MIT. Redistributable, which is why the built ROM is committed
  here rather than fetched at build time.
* **What is vendored:** `gbselftest.gb` (the built cartridge) and `CHECKS.md`
  (the check reference its failure URLs point at). The pinned release, commit
  and checksum are in `gbselftest.lock`.

## Why it is here and not in `src/`

It is consumed as an **artefact**, never as source, and that is deliberate.

This project's conformance work rests on one rule: *no check's expected output
may be defined by our emulator* ([`docs/conformance-action.md`](../../docs/conformance-action.md)).
A test cartridge kept inside the emulator it judges would be the easiest
possible way to break that rule. Keeping it in its own repository means it can
be adopted by other emulator authors without taking a dependency on us, and it
means it is free to disagree with us — which it does, on five rows, on purpose.

Vendored rather than fetched at build time and rather than a submodule: a
submodule breaks shallow checkouts and source tarballs, and a build-time fetch
puts the network on the path of `cargo test`. This is the same shape as
`third_party/dmg-acid2/`, plus the one thing that artefact does not need — a
lock, because dmg-acid2 is frozen and this one is alive.

## Using it here

```sh
tools/fetch-gbselftest.sh --verify        # offline: the ROM is the pinned one
tools/fetch-gbselftest.sh --check-upstream # advisory: is there a newer release
tools/gbselftest-gate.py                  # score both picture modes and gate
```

The check reference is spliced into
[`docs/gbselftest.md`](../../docs/gbselftest.md) by
`docs/tools/gbselftest_doc.py`, so the anchors the cartridge prints resolve.

**Do not edit anything in this directory.** The checksum in `gbselftest.lock`
is what `--verify` compares against; a hand-edited copy makes the pin a lie and
the next refresh silently reverts it. Fix the cartridge upstream, cut a
release, and re-run the fetch script.

## Refreshing

```sh
GBSELFTEST_REF=<tag> tools/fetch-gbselftest.sh
docs/tools/gbselftest_doc.py
tools/gbselftest-gate.py
```

A refresh moves what CI scores, so it belongs in a commit of its own with the
gate's output in the message.
