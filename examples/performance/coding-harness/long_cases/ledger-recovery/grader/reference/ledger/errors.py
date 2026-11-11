class LedgerError(ValueError):
    """Invalid or conflicting ledger data."""


class CorruptJournal(LedgerError):
    pass


class DuplicateEvent(LedgerError):
    pass
