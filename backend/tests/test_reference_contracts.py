from app.reference_contracts import REFERENCE_DIR, reference_drift


def test_rewrite_reference_contracts_are_current() -> None:
    assert reference_drift(REFERENCE_DIR) == []
