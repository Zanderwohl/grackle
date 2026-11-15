# /// script
# requires-python = ">=3.11"
# dependencies = ["pandas", "numpy"]
# ///
"""Usage: uv run parse.py ROUNDS_DIR... OUT.csv  (e.g. cache/rounds)

Reads round records into one row per bot per round: its temperament and what it scored."""
import json
import sys
import hashlib
from pathlib import Path

import pandas as pd


def flatten(prefix, value, out):
    if isinstance(value, dict):
        for key, inner in value.items():
            flatten(f"{prefix}.{key}" if prefix else key, inner, out)
    else:
        out[prefix] = value


def read_round(path: Path):
    start = end = None
    brains = {}
    kills = []
    damage = []
    wants = {}
    weapons = {}
    pickups = {}
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("event")
        if kind == "round_start":
            start = event
        elif kind == "player" and event.get("temperament"):
            brains[event["id"]] = event["temperament"]
        elif kind == "kill":
            kills.append(event)
        elif kind == "damage":
            damage.append(event)
        elif kind == "pickup" and event.get("by") is not None:
            pickups.setdefault(event["by"], {}).setdefault(event["what"], 0)
            pickups[event["by"]][event["what"]] += 1
        elif kind == "sample":
            for body in event["bodies"]:
                if body.get("want"):
                    wants.setdefault(body["id"], {}).setdefault(body["want"], 0)
                    wants[body["id"]][body["want"]] += 1
                if body.get("weapon"):
                    weapons.setdefault(body["id"], {}).setdefault(body["weapon"], 0)
                    weapons[body["id"]][body["weapon"]] += 1
        elif kind == "round_end":
            end = event
    if start is None or end is None or not brains:
        return []
    minutes = max(end["t"], 1.0) / 60.0
    board = {s["id"]["0"] if isinstance(s["id"], dict) else s["id"]: s for s in end["scoreboard"]}
    players = len(board)
    total_kills = sum(s["kills"] for s in board.values()) or 1
    distances = {}
    for kill in kills:
        killer = (kill.get("killer") or {}).get("id")
        if killer is not None and killer != kill["victim"]["id"] and kill.get("distance") is not None:
            distances.setdefault(killer, []).append(kill["distance"])
    rows = []
    for id, temperament in brains.items():
        standing = board.get(id)
        if standing is None:
            continue
        row = {}
        flatten("", temperament, row)
        row.pop("route.beliefs", None)
        key = json.dumps({k: v for k, v in sorted(row.items())}, sort_keys=True)
        row["brain"] = hashlib.sha1(key.encode()).hexdigest()[:10]
        row.update(
            file=str(path),
            map=start["map"],
            skill=start["bot_skill"],
            players=players,
            minutes=minutes,
            id=id,
            frags=standing["frags"],
            kills=standing["kills"],
            deaths=standing["deaths"],
            assists=standing["assists"],
            damage_dealt=standing["damage_dealt"],
            damage_taken=standing["damage_taken"],
            kill_share=standing["kills"] / total_kills * players,
            kill_distance=pd.Series(distances.get(id, [float("nan")])).median(),
        )
        for prefix, counts in (("want.", wants.get(id, {})), ("held.", weapons.get(id, {}))):
            total = sum(counts.values()) or 1
            for key, n in counts.items():
                row[prefix + key] = n / total
        for key, n in pickups.get(id, {}).items():
            row["picked." + key] = n / minutes
        rows.append(row)
    if rows:
        frags = sorted((r["frags"] for r in rows), reverse=True)
        for r in rows:
            # 1 for first, 0 for last; ties share the average place.
            better = sum(f > r["frags"] for f in frags)
            same = sum(f == r["frags"] for f in frags) - 1
            place = better + same / 2
            r["placing"] = 1 - place / (players - 1) if players > 1 else 0.5
            r["won"] = float(better == 0 and same == 0)
    return rows


def main():
    roots = [Path(p) for p in sys.argv[1:-1]]
    out = Path(sys.argv[-1])
    rows = []
    for root in roots:
        for path in sorted(root.rglob("*.jsonl")):
            rows.extend(read_round(path))
    frame = pd.DataFrame(rows)
    frame["kd_rate"] = (frame["kills"] - frame["deaths"]) / frame["minutes"]
    frame["damage_rate"] = (frame["damage_dealt"] - frame["damage_taken"]) / frame["minutes"]
    for column in frame.columns:
        if column.startswith(("want.", "held.", "picked.")):
            frame[column] = frame[column].fillna(0.0)
    frame["dealt_rate"] = frame["damage_dealt"] / frame["minutes"]
    frame["taken_rate"] = frame["damage_taken"] / frame["minutes"]
    frame.to_csv(out, index=False)
    print(f"{len(frame)} bot-rounds from {frame['file'].nunique()} rounds, {frame['brain'].nunique()} brains")
    print(frame.groupby(["map", "players"]).agg(rounds=("file", "nunique"), bots=("brain", "count")))


if __name__ == "__main__":
    main()
