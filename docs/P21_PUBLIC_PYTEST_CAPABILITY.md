# Public pytest capability check

The pinned public pytest corpus is available at tag `8.4.0`, commit
`315b3ae798fe38264b3ab2312dced212c46f1e21`. It was provisioned outside the
RTK-WAD worktree with a sparse checkout of `pyproject.toml`; Git origin and the
exact commit were verified.

No pytest performance row is published for this host. The available Python
3.12 runtime returned `No module named pytest` for both `python -m pytest
--version` and `python -m pip show pytest`.

This is an intentional evidence gap, not a failed benchmark and not an
installer defect. RTK-WAD does not install Python, pytest, or any project
dependency merely to classify or benchmark a command. A future pytest row must
use a deliberately provisioned isolated environment, record that provisioning
separately, and compare successful raw Windows, stock RTK, and WAD executions.
