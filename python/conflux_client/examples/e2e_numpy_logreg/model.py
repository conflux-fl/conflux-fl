"""Plain NumPy logistic regression — weights are already a flat vector
(`[w_1..w_d, bias]`), so no flatten/unflatten step is needed to match
Conflux's wire format (unlike Option B's real `nn.Module`). Shared by
trainer_client.py, eval_client.py, and centralized_baseline.py so all
three train/score identically.
"""

import numpy as np


def predict_proba(weights: np.ndarray, X: np.ndarray) -> np.ndarray:
    w, b = weights[:-1], weights[-1]
    z = X @ w + b
    return 1.0 / (1.0 + np.exp(-z))


def accuracy(weights: np.ndarray, X: np.ndarray, y: np.ndarray) -> float:
    pred = predict_proba(weights, X) >= 0.5
    return float(np.mean(pred == y))


def loss(weights: np.ndarray, X: np.ndarray, y: np.ndarray) -> float:
    p = np.clip(predict_proba(weights, X), 1e-7, 1 - 1e-7)
    return float(-np.mean(y * np.log(p) + (1 - y) * np.log(1 - p)))


def train_steps(weights: np.ndarray, X: np.ndarray, y: np.ndarray, lr: float, steps: int) -> np.ndarray:
    """Full-batch gradient descent, `steps` iterations, on `weights`
    (copied — the caller's array is never mutated in place)."""
    w, b = weights[:-1].copy(), weights[-1]
    for _ in range(steps):
        pred = predict_proba(np.append(w, b), X)
        grad_w = X.T @ (pred - y) / len(y)
        grad_b = np.mean(pred - y)
        w -= lr * grad_w
        b -= lr * grad_b
    return np.append(w, b).astype(np.float32)
