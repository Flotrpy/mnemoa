import pytest

from mnemoa import CLI, CLIError, MnemoaError


def test_cli_scan_and_budget(mnemoa_bin, repo):
    cli = CLI(mnemoa_bin)
    r = cli.scan(str(repo), history=False)
    assert r.summary.errors == 1
    assert r.findings[0].rule == "MNA007"
    b = cli.budget(str(repo), targets=["claude-code"])
    assert b["targets"][0]["target"] == "claude-code"
    assert any(rule["id"] == "MNA013" for rule in cli.rules())


def test_cli_promote_and_forget_dry_run(mnemoa_bin, repo):
    cli = CLI(mnemoa_bin)
    plan = cli.promote(str(repo))
    assert plan["promotions"][0]["enforcer"] == "editorconfig"
    assert not (repo / ".editorconfig").exists()
    f = cli.forget(str(repo))
    assert isinstance(f["tokens"], list)


def test_cli_errors(mnemoa_bin, tmp_path):
    with pytest.raises(CLIError):
        CLI(mnemoa_bin).scan(str(tmp_path / "missing"))


def test_missing_binary(monkeypatch):
    monkeypatch.delenv("MNEMOA_BIN", raising=False)
    monkeypatch.setenv("PATH", "")
    with pytest.raises(MnemoaError):
        CLI()
