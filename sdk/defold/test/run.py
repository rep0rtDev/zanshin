#!/usr/bin/env python3
"""Run test.lua in a Lua 5.1 runtime (lupa) with `json` provided by Python."""
import json, os, sys

try:
    import lupa
except ImportError:
    sys.exit("pip install lupa")

from lupa import lua51 as lua_mod  # noqa: E402  (Lua 5.1 like Defold)

L = lua_mod.LuaRuntime(unpack_returned_tuples=True)
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def to_py(v):
    if lua_mod.lua_type(v) == "table":
        keys = list(v.keys())
        if keys and all(isinstance(k, int) for k in keys) and sorted(keys) == list(range(1, len(keys) + 1)):
            return [to_py(v[k]) for k in sorted(keys)]
        return {str(k): to_py(v[k]) for k in keys}
    return v


def to_lua(v):
    if isinstance(v, dict):
        t = L.table()
        for k, x in v.items():
            t[k] = to_lua(x)
        return t
    if isinstance(v, list):
        t = L.table()
        for i, x in enumerate(v, 1):
            t[i] = to_lua(x)
        return t
    return v


json_tbl = L.table()
json_tbl["encode"] = lambda v: json.dumps(to_py(v))
json_tbl["decode"] = lambda s: to_lua(json.loads(s))
L.globals()["json"] = json_tbl
L.globals()["package"]["path"] = f"{root}/?.lua;{root}/?/init.lua;" + L.globals()["package"]["path"]
L.execute(open(os.path.join(root, "test", "test.lua")).read())
