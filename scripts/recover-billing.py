#!/usr/bin/env python3
"""Dry-run billing recovery from audited, exact-capture evidence. No event writes.

See docs/billing-recovery.md for the evidence contract and review requirements.
"""
import argparse
from collections import Counter, defaultdict
from hashlib import sha256
import json
from pathlib import Path
import re
from uuid import UUID

IDENTITY = ('id', 'thread_id', 'sequence', 'original_event_sha256', 'request_event_id')
MODES = {'claude-gateway': 'unknown', 'vertex': 'token-metered',
         'bedrock': 'token-metered', 'foundry': 'token-metered',
         'anthropic': 'token-metered', 'anthropic-subscription': 'plan',
         'openai': 'token-metered', 'chatgpt-plan': 'plan'}
SOURCES = {'runtime-observation', 'session-metadata', 'call-metadata'}
USAGE_FIELDS = {'input_tokens', 'output_tokens', 'cache_read_tokens',
                'cache_creation_tokens', 'cache_creation_1h_tokens',
                'cache_creation_30m_tokens', 'cache_creation_12h_tokens'}
USAGE_METADATA = {'modality', 'service_tier', 'cache_creation_requested_ttl'}


def digest(value):
    return sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def require(condition):
    if not condition:
        raise ValueError('Invalid recovery input; see the documented schema')


def valid_hash(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def identity(row):
    result = {k: row[k] for k in IDENTITY}
    for key in ('id', 'thread_id'):
        UUID(result[key])
    if result['request_event_id'] is not None:
        UUID(result['request_event_id'])
    require(type(result['sequence']) is int and result['sequence'] >= 0)
    require(valid_hash(result['original_event_sha256']))
    return result


def recover(snapshot, evidence):
    """Accept exact capture assertions only; never infer across a session/date."""
    before = digest(snapshot)
    require(type(evidence['schema_version']) is int and evidence['schema_version'] == 1)
    require(isinstance(evidence['observations'], list))
    captures = snapshot['captures']
    require(isinstance(captures, list))
    indexed = {}
    tokens = Counter()
    for row in captures:
        key = identity(row)['id']
        require(key not in indexed)
        require(row['producer'] in ('claude_code', 'codex'))
        require(row.get('provider') in (None, 'unknown'))
        indexed[key] = row
        usage = row.get('usage') or {}
        require(isinstance(usage, dict) and set(usage) <= USAGE_FIELDS | USAGE_METADATA)
        for field in USAGE_FIELDS & usage.keys():
            value = usage[field]
            require(type(value) is int and value >= 0)
            tokens[field] += value
    matched = defaultdict(list)
    for item in evidence['observations']:
        bound = identity(item['capture'])
        row = indexed.get(bound['id'])
        require(row is not None and identity(row) == bound)
        require(item['producer'] == row['producer'])
        require(item['scope'] == 'exact-capture')
        require(item['source'] in SOURCES and valid_hash(item['source_sha256']))
        require(item['status'] in ('known', 'transport-only', 'unresolved'))
        if item['status'] == 'known':
            provider = item['provider']
            require(provider in MODES and item['billing_mode'] == MODES[provider])
            require(provider in ({'openai', 'chatgpt-plan'} if row['producer'] == 'codex'
                                 else set(MODES) - {'openai', 'chatgpt-plan'}))
        matched[bound['id']].append(item)
    proven, partial, unresolved = [], [], []
    counts = defaultdict(Counter)
    for key, row in indexed.items():
        items = matched[key]
        known = {(x['provider'], x['billing_mode']) for x in items if x['status'] == 'known'}
        result = {'capture': identity(row)}
        # An unresolved/transport-only record at the same capture can represent
        # changed auth. It must never be overridden by another known assertion.
        if len(known) > 1 or (known and any(x['status'] != 'known' for x in items)):
            status, reason = 'unresolved', 'conflicting-evidence'
        elif len(known) == 1:
            provider, mode = next(iter(known))
            provenance = {json.dumps({k: x[k] for k in ('source', 'source_sha256', 'scope')},
                                     sort_keys=True) for x in items}
            result.update(provider=provider, billing_mode=mode,
                          provenance=[json.loads(x) for x in sorted(provenance)])
            proven.append(result)
            status, reason = 'proven', None
        elif any(x['status'] == 'transport-only' for x in items):
            status, reason = 'partial', 'transport-without-billing-auth'
        else:
            status, reason = 'unresolved', 'insufficient-evidence' if items else 'no-bound-evidence'
        counts[row['producer']][status] += 1
        if reason:
            result['reason'] = reason
            (partial if status == 'partial' else unresolved).append(result)
    require(before == digest(snapshot))
    return {'schema_version': 1, 'snapshot_sha256': before,
            'evidence_sha256': digest(evidence), 'proven': sorted(proven, key=lambda x: x['capture']['id']),
            'partial': sorted(partial, key=lambda x: x['capture']['id']),
            'unresolved': sorted(unresolved, key=lambda x: x['capture']['id']),
            'coverage': {k: dict(sorted(v.items())) for k, v in sorted(counts.items())},
            'parity': {'capture_count': len(captures), 'usage_totals': dict(sorted(tokens.items())),
                       'originals_unchanged': True,
                       'cost_effect': 'none; attribution overlay only; no repricing or added usage'}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--captures', type=Path, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--output', type=Path, help='Write sidecar; omitted means dry-run summary only')
    args = parser.parse_args()
    try:
        snapshot_raw = args.captures.read_bytes()
        snapshot = json.loads(snapshot_raw)
        evidence = json.loads(args.evidence.read_text())
        result = recover(snapshot, evidence)
        result['snapshot_file_sha256'] = sha256(snapshot_raw).hexdigest()
        if args.output:
            # Exclusive creation prevents accidentally replacing inputs or past reports.
            with args.output.open('x') as output:
                json.dump(result, output, sort_keys=True, indent=2)
                output.write('\n')
        print(json.dumps({k: result[k] for k in ('schema_version', 'coverage', 'parity')}, sort_keys=True))
    except (ValueError, KeyError, TypeError, AttributeError, OSError):
        parser.error('Recovery failed: invalid input or unreadable/existing output; no events changed')


if __name__ == '__main__':
    main()
