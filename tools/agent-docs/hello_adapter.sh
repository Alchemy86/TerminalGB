#!/bin/sh
# The smallest possible driver: read the state snapshot on stdin, print a plan
# on stdout, exit 0. That is the whole adapter contract — see
# `docs/agent-reference.md` § "Writing a driver".
#
# This one ignores what it was told and presses A for ever, which is enough to
# get through a title screen and any amount of dialogue. It is here to be read,
# and to be the first thing you run when wiring up your own:
#
#   GB_PLUGINS=agent GB_AGENT_CMD=tools/agent-docs/hello_adapter.sh \
#     cargo run --release --example terminal -- <rom>
#
# The host clamps every number in the reply, so a bug here cannot make the
# emulator do anything a player could not.
cat > /dev/null
printf '%s\n' '{"goal":"Press A until something happens",
                "say":"Pressing A. It is a strategy.",
                "plan":[{"do":"press","button":"a","times":8}]}'
