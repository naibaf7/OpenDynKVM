"""Minimal KiCad S-expression reader/writer used by the OpenRower hardware generators."""

import re


class Q(str):
    """A quoted string atom."""


_TOKEN = re.compile(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()]+')


def parse(text):
    stack = [[]]
    for tok in _TOKEN.findall(text):
        if tok == '(':
            stack.append([])
        elif tok == ')':
            node = stack.pop()
            stack[-1].append(node)
        elif tok.startswith('"'):
            stack[-1].append(Q(tok[1:-1].replace('\\"', '"').replace('\\\\', '\\')))
        else:
            stack[-1].append(tok)
    return stack[0][0]


def _atom(a):
    if isinstance(a, Q):
        return '"' + a.replace('\\', '\\\\').replace('"', '\\"') + '"'
    if isinstance(a, float):
        s = f'{a:.4f}'.rstrip('0').rstrip('.')
        return '0' if s in ('-0', '') else s
    return str(a)


def dump(node, indent=0):
    """Serialize with one nested list per line, like KiCad does."""
    if not isinstance(node, list):
        return _atom(node)
    head = []
    tail = []
    for item in node:
        (tail if isinstance(item, list) else head).append(item) if not tail else tail.append(item)
    out = '(' + ' '.join(_atom(a) for a in head)
    if not tail:
        return out + ')'
    pad = '\t' * (indent + 1)
    for item in tail:
        out += '\n' + pad + (dump(item, indent + 1) if isinstance(item, list) else _atom(item))
    return out + '\n' + '\t' * indent + ')'


def find(node, tag):
    return [c for c in node if isinstance(c, list) and c and c[0] == tag]


def first(node, tag):
    r = find(node, tag)
    return r[0] if r else None


def prop(node, name):
    for p in find(node, 'property'):
        if p[1] == name:
            return p
    return None


def board_stackup(path):
    """A .kicad_pcb's physical stack-up as [(layer, type, thickness mm or None, epsilon_r or None)], top to bottom,
    read from its setup without parsing the whole board."""
    text = open(path).read()
    i = text.find('(stackup')
    if i < 0:
        return []
    depth = 0
    for j in range(i, len(text)):
        depth += {'(': 1, ')': -1}.get(text[j], 0)
        if depth == 0:
            break
    out = []
    for layer in find(parse(text[i:j + 1]), 'layer'):
        val = {tag: (first(layer, tag) or [None, None])[1] for tag in ('type', 'thickness', 'epsilon_r')}
        out.append((str(layer[1]), str(val['type']), float(val['thickness']) if val['thickness'] else None,
                    float(val['epsilon_r']) if val['epsilon_r'] else None))
    return out
