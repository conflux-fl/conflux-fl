#!/usr/bin/env python3
"""Downloads the Shakespeare corpus and partitions it across N federated
clients **by speaking role** — the partition LEAF's Shakespeare benchmark
uses, and the reason this harness exists.

Every other harness here has to *synthesize* non-IID-ness, usually with a
Dirichlet label skew whose severity is a knob someone chose. Shakespeare
doesn't: each speaking role is a different person with a different
vocabulary, cadence, and subject matter, so partitioning by role produces
a federation that is non-IID *because of what the data is*. A fairness or
robustness result measured against a Dirichlet knob is partly a result
about the knob; measured here, it isn't.

Writes, into --out-dir:
  shard_0.pt .. shard_{N-1}.pt   one role's text each (or an IID split)
  held_out.pt                     never partitioned, for eval_client.py
  pooled.pt                       every shard concatenated, for
                                   centralized_baseline.py's comparison
  vocab.pt                        the character alphabet every process
                                   must agree on
"""

import argparse
import re
import urllib.request
from collections import defaultdict
from pathlib import Path

import numpy as np
import torch

from model import SEQ_LEN, build_vocab

# The tiny-shakespeare corpus: the Complete Works, concatenated, with
# speaking roles marked as "NAME:" on their own line. ~1.1 MB, which is
# what makes a by-role partition cheap enough to run in a demo.
CORPUS_URL = (
    "https://raw.githubusercontent.com/karpathy/char-rnn/"
    "master/data/tinyshakespeare/input.txt"
)
ROLE_RE = re.compile(r"^([A-Z][A-Za-z ]+):$", re.M)


def fetch_corpus(cache_path: Path) -> str:
    """Downloads once and caches, matching how the torchvision harnesses
    treat their own datasets."""
    if cache_path.exists():
        return cache_path.read_text(encoding="utf-8")
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    print(f"downloading corpus -> {cache_path}")
    text = urllib.request.urlopen(CORPUS_URL, timeout=60).read().decode("utf-8")
    cache_path.write_text(text, encoding="utf-8")
    return text


def split_by_role(text: str) -> dict[str, str]:
    """Maps each speaking role to all of its lines, concatenated.

    Everything before the first role marker (stage directions, front
    matter) is discarded rather than assigned to some arbitrary role —
    it belongs to no speaker, and silently attributing it would blur
    exactly the per-client distinction this partition exists to create.
    """
    roles: dict[str, list[str]] = defaultdict(list)
    matches = list(ROLE_RE.finditer(text))
    for i, m in enumerate(matches):
        end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        roles[m.group(1)].append(text[m.end() : end])
    return {role: "".join(parts) for role, parts in roles.items()}


def to_sequences(text: str, stoi: dict[str, int], max_samples: int, seed: int):
    """Turns a block of text into (context, next-character) pairs."""
    if len(text) <= SEQ_LEN + 1:
        return torch.empty(0, SEQ_LEN, dtype=torch.long), torch.empty(0, dtype=torch.long)
    idx = np.array([stoi[c] for c in text], dtype=np.int64)
    starts = np.arange(len(idx) - SEQ_LEN - 1)
    if len(starts) > max_samples:
        starts = np.random.default_rng(seed).choice(starts, size=max_samples, replace=False)
    X = np.stack([idx[s : s + SEQ_LEN] for s in starts])
    y = idx[starts + SEQ_LEN]
    return torch.from_numpy(X), torch.from_numpy(y)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--n-clients", type=int, default=5)
    parser.add_argument(
        "--split",
        choices=["role", "iid"],
        default="role",
        help="'role' is the natural federated partition (one speaking role per client, "
        "non-IID by construction); 'iid' pools every role and splits randomly, as the "
        "controlled comparison",
    )
    parser.add_argument(
        "--per-client-samples",
        type=int,
        default=800,
        help="cap per client, to keep a demo round's wall-clock reasonable",
    )
    parser.add_argument("--n-held-out", type=int, default=1000)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--data-root", default="/tmp/conflux_shakespeare")
    parser.add_argument("--out-dir", default=".")
    args = parser.parse_args()

    torch.manual_seed(args.seed)
    text = fetch_corpus(Path(args.data_root) / "input.txt")

    # Vocabulary from the *whole* corpus, not from any one client's
    # shard: a client whose lines happen never to use 'z' must still
    # agree with everyone else about what index 'z' has, or the models
    # are not the same model.
    vocab = build_vocab(text)
    stoi = {c: i for i, c in enumerate(vocab)}
    torch.save({"vocab": vocab}, f"{args.out_dir}/vocab.pt")
    print(f"wrote vocab.pt: {len(vocab)} characters")

    by_role = split_by_role(text)
    # Largest roles first: a client needs enough text to train on at all,
    # and the long tail of one-line roles would produce empty shards.
    ranked = sorted(by_role.items(), key=lambda kv: len(kv[1]), reverse=True)

    if args.split == "role":
        chosen = ranked[: args.n_clients]
        if len(chosen) < args.n_clients:
            raise SystemExit(f"corpus has only {len(chosen)} usable roles")
        shards = []
        for i, (role, role_text) in enumerate(chosen):
            X, y = to_sequences(role_text, stoi, args.per_client_samples, args.seed + i)
            shards.append((role, X, y))
    else:
        pooled_text = "".join(t for _, t in ranked[: max(args.n_clients * 4, 20)])
        X, y = to_sequences(
            pooled_text, stoi, args.per_client_samples * args.n_clients, args.seed
        )
        perm = torch.randperm(len(X))
        X, y = X[perm], y[perm]
        parts = torch.chunk(torch.arange(len(X)), args.n_clients)
        shards = [(f"iid-{i}", X[p], y[p]) for i, p in enumerate(parts)]

    for i, (label, X, y) in enumerate(shards):
        path = f"{args.out_dir}/shard_{i}.pt"
        torch.save({"X": X, "y": y}, path)
        print(f"wrote {path}: {len(X)} samples (role={label!r})")

    # Held-out text comes from roles *outside* the training clients, so
    # the evaluation measures the global model's general Shakespeare,
    # not how well it memorized the specific speakers it trained on.
    held_text = "".join(t for _, t in ranked[args.n_clients : args.n_clients + 20])
    X_held, y_held = to_sequences(held_text, stoi, args.n_held_out, args.seed + 999)
    torch.save({"X": X_held, "y": y_held}, f"{args.out_dir}/held_out.pt")
    print(f"wrote held_out.pt: {len(X_held)} samples (from roles no client trains on)")

    X_pool = torch.cat([X for _, X, _ in shards])
    y_pool = torch.cat([y for _, _, y in shards])
    torch.save({"X": X_pool, "y": y_pool}, f"{args.out_dir}/pooled.pt")
    print(f"wrote pooled.pt: {len(X_pool)} samples (for the centralized baseline)")


if __name__ == "__main__":
    main()
