import os
import pathlib
import shutil
import socket
import subprocess
import time

import pytest

REPO = pathlib.Path(__file__).resolve().parents[3]


def _binary(name: str, env: str):
    explicit = os.environ.get(env)
    if explicit:
        return explicit
    for profile in ("debug", "release"):
        p = REPO / "target" / profile / name
        if p.exists():
            return str(p)
    return shutil.which(name)


@pytest.fixture(scope="session")
def mnemoa_bin():
    b = _binary("mnemoa", "MNEMOA_BIN")
    if not b:
        pytest.skip("mnemoa binary not built (cargo build -p mnemoa-cli)")
    return b


@pytest.fixture()
def repo(tmp_path):
    r = tmp_path / "repo"
    r.mkdir()
    (r / "AGENTS.md").write_text("- Read [x](missing.md) before releases.\n- Use 2 spaces for indentation.\n")
    return r


@pytest.fixture()
def server(tmp_path, repo):
    api = _binary("mnemoa-api", "MNEMOA_API_BIN")
    if not api:
        pytest.skip("mnemoa-api binary not built (cargo build -p mnemoa-api)")
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    env = dict(os.environ, MNEMOA_ROOT=str(tmp_path), MNEMOA_PORT=str(port), RUST_LOG="warn")
    proc = subprocess.Popen([api], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    url = f"http://127.0.0.1:{port}"
    deadline = time.time() + 15
    while time.time() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                break
        except OSError:
            time.sleep(0.05)
    else:
        proc.kill()
        pytest.fail("mnemoa-api did not start")
    yield url
    proc.terminate()
    proc.wait(timeout=10)
