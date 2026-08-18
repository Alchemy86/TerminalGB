#!/usr/bin/env python3
"""Two players for the move-learning prompt, and the training that makes one.

Both speak the emulator's own adapter contract — one snapshot object on stdin,
one reply object on stdout, exit 0 — so either can be a `command` arm of the
A/B test rig (`docs/agent/test-rig.md`) with no glue.

    policy.py teacher --corpus <agent.ndjson> [--corpus ...]
        Replay what the drill's teacher pressed, matched on the screen. This is
        the teacher by construction: the same buttons, in the same order, on
        the same screens, all nine of which the drill verified against the
        cartridge. It is not a re-derivation of the criterion — the criterion
        lives in `tools/gen1-tables/learn_table.py` and only there.

    policy.py train --corpus <agent.ndjson> [--corpus ...] --out <model.json>
        Fit the student from the corpus.

    policy.py student --model <model.json>
        Play with what was fitted.

**The student is shown only what a player is shown.** Every feature comes from
`observation.screen`; the `label` block the drill writes beside it is ground
truth and is never read — asserted, not promised (see `_forbid_label`).

Stdlib only, and no cartridge: the whole point is that the student is trained on
the corpus rather than on the move table.
"""

import argparse
import json
import re
import sys

# -- what a screen is -------------------------------------------------------

# **Match the phrase as it looks while it is still being typed.** Gen 1 prints a
# box one character at a time, so the settled screen and the screen four frames
# earlier are different strings — and the earlier one is the one an adapter is
# most likely to be handed. Waiting for `which move should be forgotten?` and
# getting `Which move` is not a near miss: it fell through to "this is not the
# question", pressed A to read on, and forgot the move the cursor happened to
# be sitting on. Measured, on the very first prompt.
DELETE = "make room"
WHICH = "which move"
ABANDON = "abandon learn"


def norm(text):
    """A screen key that survives the numbers on it.

    The party's HP is printed beside the box (`69`, `91`, `142`), so two
    otherwise identical screens differ by digits that say nothing about the
    question. Everything else is kept verbatim, cursor included, because
    *which option is highlighted* is part of what the player is looking at.
    """
    return re.sub(r"\d+", "#", " ".join(text.split()))


def offered(text):
    """`... move to make room for RAGE?` / `Abandon learning RAGE?` -> RAGE."""
    flat = " ".join(text.split())
    m = re.search(r"(?:make room for|Abandon learning)\s+(.+?)\?", flat, re.I)
    return m.group(1).strip().upper() if m else None


def move_list(text):
    """The four move names on a `Which move should be forgotten?` screen."""
    out = []
    for line in text.splitlines():
        s = line.strip()
        if not s:
            continue
        s = s.lstrip("▶").strip()
        low = s.lower()
        if low.startswith("which move") or low.startswith("be forgotten"):
            continue
        out.append(s.upper())
    return out[:4]


def kind(screen):
    flat = " ".join(screen.get("text", "").split()).lower()
    if WHICH in flat:
        return "which"
    if ABANDON in flat:
        return "abandon"
    if DELETE in flat:
        return "delete"
    return None


def _forbid_label(obs):
    """The student may not read the ground truth. Loud, not polite."""
    if "label" in obs:
        raise SystemExit(
            "policy.py: the student was handed a `label` block. That is the "
            "drill's ground truth, not something a player sees; refusing "
            "rather than quietly scoring a cheat."
        )


# -- replies ----------------------------------------------------------------


def press(button, say, goal="Answer the move-learning prompt."):
    return {
        "goal": goal,
        "say": say,
        "plan": [{"do": "press", "button": button, "times": 1}],
        "usage": {"input_tokens": 0, "output_tokens": 0, "usd": 0.0},
    }


def wait(say):
    return {
        "goal": "Answer the move-learning prompt.",
        "say": say,
        "plan": [{"do": "wait", "frames": 30}],
        "usage": {"input_tokens": 0, "output_tokens": 0, "usd": 0.0},
    }


def read_on(screen, say="Reading through what it says."):
    """Neither answering nor waiting: getting through the text.

    **The answer does not land until the text after it has been read.** The
    game prints `LEARNER forgot LEER!` and `LEARNER learned RAGE!` and only
    then writes the party, so an adapter that sits on those boxes leaves the
    move set exactly as it found it — measured: every arm scored the pool's
    own `held` values and the prompt looked unanswered when it had in fact been
    answered correctly six decisions earlier.

    It is safe to give both players, because `advance_text` refuses to answer a
    question: it stops at a yes/no box and at the naming keyboard. And it is
    only ever reached here when none of the three prompt phrases is on screen.
    """
    return {
        "goal": "Answer the move-learning prompt.",
        "say": say,
        "plan": [{"do": "advance_text"}],
        "usage": {"input_tokens": 0, "output_tokens": 0, "usd": 0.0},
    }


def not_a_prompt(screen, say):
    """What to do on a screen that is not asking the question.

    **A menu with something highlighted is a question, whatever its words**, so
    it is waited on rather than read through: `advance_text` presses A, and A on
    a list is a choice. Only a plain box of text is read on from.
    """
    if screen.get("selected"):
        return wait("Something is highlighted and this is not the prompt; waiting.")
    if screen.get("kind") in ("dialogue", "text"):
        return read_on(screen, say)
    return wait(say)


def toward(screen, target, say):
    """One key towards `target`, read back off the cursor rather than counted.

    The drill's own hardest-won lesson: a tap shorter than the menu delay is
    swallowed, and on a four-move list a swallowed press forgets a different
    move with no complaint. Pressing while the cursor is not where you want it
    is self-correcting; counting presses is not.
    """
    sel = (screen.get("selected") or "").strip().upper()
    if sel == target.upper():
        return press("a", say)
    return press("down", f"The cursor is on {sel or 'nothing'}; moving to {target}.")


# -- the teacher ------------------------------------------------------------


def load_corpus(paths):
    rows = []
    for p in paths:
        with open(p) as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                rec = json.loads(line)
                rows.append(
                    (json.loads(rec["request"]), json.loads(rec["reply"]))
                )
    return rows


def teacher_table(rows):
    table = {}
    for obs, reply in rows:
        table[norm(obs["screen"]["text"])] = reply["plan"]
    return table


def teacher_act(table, obs):
    screen = obs.get("screen", {})
    text = screen.get("text", "")
    plan = table.get(norm(text))
    if plan is None:
        k = kind(screen)
        if k is None:
            return not_a_prompt(screen, "Not the question; reading on.")
        if not screen.get("selected"):
            if k == "delete":
                return press("a", "The question is still printing; reading on.")
            return wait("The box is still printing; waiting for the cursor.")
        # A screen the drill never stood on. Say so rather than guessing: an
        # unexplained press here would be scored as the teacher's answer.
        return wait("This screen is not in the corpus; not guessing at it.")
    return {
        "goal": "Answer the move-learning prompt.",
        "say": "Replaying what the drill's teacher pressed here.",
        "plan": plan,
        "usage": {"input_tokens": 0, "output_tokens": 0, "usd": 0.0},
    }


# -- training ---------------------------------------------------------------


def fit(rows):
    """Count, from the screens and the buttons, what the teacher tends to do.

    Three questions, and each is fitted from the only evidence a player has:

    * at `Delete an older move to make room for X?` the only thing on the
      screen is **X**, so the model is P(replace | X) with a base rate to fall
      back on. What is held is *not* on that screen — that is the cartridge's
      doing, not a simplification here.
    * at `Which move should be forgotten?` the screen is four names, so the
      model is a per-name propensity to be the one dropped, plus which slot
      tends to be chosen as a tie-break.
    * at `Abandon learning X?` the model is simply what the teacher answered.
    """
    m = {
        "replace_by_offered": {},
        "replace_base": {"yes": 0, "no": 0},
        "drop": {},
        "slot": [0, 0, 0, 0],
        "abandon": {"yes": 0, "no": 0},
        "trained_on": [],
    }
    for obs, reply in rows:
        screen = obs["screen"]
        text = screen.get("text", "")
        k = kind(screen)
        sel = (screen.get("selected") or "").strip().upper()
        buttons = [s.get("button") for s in reply["plan"] if s.get("do") == "press"]
        confirmed = "a" in buttons
        if k == "delete":
            x = offered(text)
            if x and confirmed:
                # The cursor sat on the answer when A was pressed, so `sel` IS
                # the teacher's answer to this box.
                bucket = m["replace_by_offered"].setdefault(x, {"yes": 0, "no": 0})
                key = "yes" if sel == "YES" else "no"
                bucket[key] += 1
                m["replace_base"][key] += 1
        elif k == "which":
            names = move_list(text)
            if confirmed and sel in names:
                for i, n in enumerate(names):
                    d = m["drop"].setdefault(n, {"dropped": 0, "kept": 0})
                    d["dropped" if n == sel else "kept"] += 1
                m["slot"][names.index(sel)] += 1
        elif k == "abandon":
            if confirmed:
                m["abandon"]["yes" if sel == "YES" else "no"] += 1
    return m


def rate(bucket, a, b):
    """Laplace, so a name seen once does not become a certainty."""
    return (bucket[a] + 1.0) / (bucket[a] + bucket[b] + 2.0)


def student_act(m, obs):
    _forbid_label(obs)
    screen = obs.get("screen", {})
    if screen.get("trust") not in (None, "ok"):
        return wait("The screen is mid-transition; waiting.")
    text = screen.get("text", "")
    k = kind(screen)
    if k is None:
        return not_a_prompt(screen, "No prompt on screen; reading on.")

    if k == "delete":
        if not screen.get("selected"):
            # A on this box advances the text and cannot answer it — the same
            # reading the drill's own Delete arm makes.
            return press("a", "The question is still printing; reading on.")
        x = offered(text) or ""
        seen = m["replace_by_offered"].get(x)
        p = rate(seen, "yes", "no") if seen else rate(m["replace_base"], "yes", "no")
        target = "YES" if p >= 0.5 else "NO"
        note = "seen in training" if seen else "never seen; using the base rate"
        return toward(screen, target, f"{x}: replace with p={p:.2f} ({note}) -> {target}.")

    if k in ("which", "abandon") and not screen.get("selected"):
        # The box is still typing itself out. The drill taps A through the
        # DELETE box's typing because A there only advances text; on a list it
        # would choose. So: wait.
        return wait("The box is still printing; waiting for the cursor.")

    if k == "abandon":
        p = rate(m["abandon"], "yes", "no")
        target = "YES" if p >= 0.5 else "NO"
        return toward(screen, target, f"Abandon box: {target} (p={p:.2f}).")

    names = move_list(text)
    if not names:
        return wait("Could not read the move list; waiting.")
    best, best_score = None, None
    for i, n in enumerate(names):
        d = m["drop"].get(n)
        s = rate(d, "dropped", "kept") if d else 0.5
        # The learned slot preference is the tie-break, worth a hair so it can
        # only decide between names the corpus cannot separate.
        total = sum(m["slot"]) or 1
        s += 0.001 * (m["slot"][i] / total)
        if best_score is None or s > best_score:
            best, best_score = n, s
    return toward(screen, best, f"Dropping {best} (score {best_score:.3f}).")


# -- main -------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(add_help=True)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("teacher")
    t.add_argument("--corpus", action="append", required=True)
    tr = sub.add_parser("train")
    tr.add_argument("--corpus", action="append", required=True)
    tr.add_argument("--out", required=True)
    st = sub.add_parser("student")
    st.add_argument("--model", required=True)
    a = ap.parse_args()

    if a.cmd == "train":
        rows = load_corpus(a.corpus)
        m = fit(rows)
        m["trained_on"] = list(a.corpus)
        m["rows"] = len(rows)
        with open(a.out, "w") as fh:
            json.dump(m, fh, indent=1, sort_keys=True)
        print(f"model: {a.out}")
        print(f"  rows            {len(rows)}")
        print(f"  offered moves   {len(m['replace_by_offered'])}")
        print(f"  move names      {len(m['drop'])}")
        print(f"  slot counts     {m['slot']}")
        return 0

    raw = sys.stdin.read()
    try:
        obs = json.loads(raw)
    except json.JSONDecodeError:
        print("policy.py: could not parse the snapshot", file=sys.stderr)
        return 1
    if a.cmd == "teacher":
        reply = teacher_act(teacher_table(load_corpus(a.corpus)), obs)
    else:
        with open(a.model) as fh:
            reply = student_act(json.load(fh), obs)
    json.dump(reply, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
