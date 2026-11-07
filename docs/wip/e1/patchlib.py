"""Parches exactos sobre archivos de la worktree, con soporte de CRLF e idempotencia."""

ROOT = "D:/Personales/Repos/browser-freedom-army-e1/"


def patch(name, pairs):
    p = ROOT + name
    s = open(p, encoding="utf-8", newline="").read()
    crlf = "\r\n" in s
    for a, b in pairs:
        if crlf:
            a = a.replace("\r\n", "\n").replace("\n", "\r\n")
            b = b.replace("\r\n", "\n").replace("\n", "\r\n")
        if b in s and a not in s:
            continue
        assert s.count(a) == 1, (name, a[:200], s.count(a))
        s = s.replace(a, b, 1)
    open(p, "w", encoding="utf-8", newline="").write(s)
