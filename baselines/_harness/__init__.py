"""The shared training harness the baselines run on.

Before this package, each `e2e_*` example carried its own `model.py`,
`partition_data.py`, `trainer_client.py` and `eval_client.py`, and the
CIFAR-10 copies were, in the words of the note that asked for this,
"copied over unchanged from MNIST". Four near-identical copies is four
places for a fix to be applied three times.

What actually differs between harnesses is small: the architecture, how
the dataset loads, and how it is split across clients. Everything else —
flattening a model into the wire's `f32` vector, recognizing the
server's all-zero placeholder, running local SGD steps, scoring a
checkpoint — is architecture-independent and now lives once, in
[`torch_model`].

A baseline therefore names a *recipe* rather than a directory:

    [experiment]
    model     = "mlp"
    dataset   = "mnist"
    partition = "iid"

and `conflux-baselines` builds the pieces from the registries in
[`models`] and [`datasets`].
"""
