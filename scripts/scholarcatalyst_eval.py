#!/usr/bin/env python3
"""Prepare gold-free tasks and score ScholarCatalyst offline retrieval runs."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import sys
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Set, Tuple


SCHEMA_VERSION = 1
QUERY_TYPES = ("core_query", "subfield_query")
TYPE_FILES = {"core_query": "core_query.jsonl", "subfield_query": "subfield_query.jsonl"}
_TITLE_NORM = re.compile(r"[^a-z0-9]+")
_ISO_MONTH = re.compile(r"^(\d{4})-(\d{2})")
_YEAR = re.compile(r"^(\d{4})$")
_ARXIV_ID = re.compile(r"^arxiv_(\d{2})(\d{2})\.\d+")


class EvaluationError(Exception):
    """An invalid benchmark or run input."""


def fail(message: str) -> None:
    raise EvaluationError(message)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def ensure_output_distinct(output: Path, inputs: Sequence[Path]) -> None:
    output_resolved = output.resolve()
    for input_path in inputs:
        if output_resolved == input_path.resolve():
            fail("output path must not overwrite an input file: {}".format(input_path))
        if output.exists() and input_path.exists() and os.path.samefile(output, input_path):
            fail("output path must not overwrite an input file: {}".format(input_path))


def read_jsonl(path: Path) -> Iterable[Tuple[int, Dict[str, Any]]]:
    if not path.is_file():
        fail("required file does not exist: {}".format(path))
    with path.open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            if not line.strip():
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as error:
                fail("{}:{}: invalid JSON: {}".format(path, line_number, error.msg))
            if not isinstance(value, dict):
                fail("{}:{}: each JSONL row must be an object".format(path, line_number))
            yield line_number, value


def require_string(row: Dict[str, Any], key: str, context: str, optional: bool = False) -> str:
    value = row.get(key)
    if value is None and optional:
        return ""
    if not isinstance(value, str):
        fail("{}: '{}' must be a string{}".format(
            context, key, " when present" if optional else ""))
    return value


def normalize_title(title: str) -> str:
    return _TITLE_NORM.sub(" ", title.lower()).strip()


def parse_month(value: str, doc_id: str = "") -> Optional[Tuple[int, int]]:
    """Match the official evaluator: ISO year-month, year-only month zero, then arXiv ID fallback."""
    if value:
        match = _ISO_MONTH.match(value)
        if match:
            month = int(match.group(2))
            if 1 <= month <= 12:
                return int(match.group(1)), month
            return None
        match = _YEAR.match(value)
        if match:
            return int(match.group(1)), 0
        return None
    match = _ARXIV_ID.match(doc_id)
    if match:
        short_year, month = int(match.group(1)), int(match.group(2))
        if 1 <= month <= 12:
            return (2000 + short_year if short_year <= 30 else 1900 + short_year), month
    return None


def is_after_cutoff(published: str, doc_id: str, cutoff: str) -> bool:
    cutoff_month = parse_month(cutoff)
    document_month = parse_month(published, doc_id)
    if cutoff_month is None or document_month is None:
        return False
    year, month = document_month
    cutoff_year, cutoff_month_number = cutoff_month
    if year != cutoff_year:
        return year > cutoff_year
    return month != 0 and cutoff_month_number != 0 and month > cutoff_month_number


def load_corpus(path: Path) -> Dict[str, Dict[str, str]]:
    """Stream corpus rows and retain only id, title, and published metadata."""
    corpus: Dict[str, Dict[str, str]] = {}
    for line_number, row in read_jsonl(path):
        context = "{}:{}".format(path, line_number)
        doc_id = require_string(row, "id", context)
        if not doc_id:
            fail("{}: 'id' must not be empty".format(context))
        if doc_id in corpus:
            fail("{}: duplicate corpus id '{}'".format(context, doc_id))
        corpus[doc_id] = {
            "title": require_string(row, "title", context, optional=True),
            "published": require_string(row, "published", context, optional=True),
        }
    if not corpus:
        fail("{}: corpus is empty".format(path))
    return corpus


def load_queries(path: Path) -> Tuple[List[Dict[str, str]], Dict[str, Dict[str, str]]]:
    queries: List[Dict[str, str]] = []
    by_id: Dict[str, Dict[str, str]] = {}
    for line_number, row in read_jsonl(path):
        context = "{}:{}".format(path, line_number)
        query_id = require_string(row, "id", context)
        query_type = require_string(row, "type", context)
        if not query_id:
            fail("{}: 'id' must not be empty".format(context))
        if query_id in by_id:
            fail("{}: duplicate query id '{}'".format(context, query_id))
        if query_type not in QUERY_TYPES:
            fail("{}: unknown query type '{}'".format(context, query_type))
        query = {
            "id": query_id,
            "type": query_type,
            "question": require_string(row, "question", context),
            "paper_id": require_string(row, "paper_id", context, optional=True),
            "paper_title": require_string(row, "paper_title", context, optional=True),
            "cutoff": require_string(row, "paper_published", context, optional=True),
        }
        queries.append(query)
        by_id[query_id] = query
    return queries, by_id


def load_relations(bench_dir: Path, query_type: str, query_by_id: Dict[str, Dict[str, str]],
                   corpus: Dict[str, Dict[str, str]]) -> Tuple[Dict[str, Set[str]], Path]:
    path = bench_dir / "rels" / TYPE_FILES[query_type]
    if not path.is_file():
        fail("required relation file does not exist: {}".format(path))
    relations: Dict[str, Set[str]] = {}
    for line_number, row in read_jsonl(path):
        context = "{}:{}".format(path, line_number)
        query_id = require_string(row, "query_id", context)
        if query_id in relations:
            fail("{}: duplicate relation query id '{}'".format(context, query_id))
        if query_id not in query_by_id:
            fail("{}: unknown relation query id '{}'".format(context, query_id))
        if query_by_id[query_id]["type"] != query_type:
            fail("{}: relation query '{}' has the wrong type".format(context, query_id))
        positives = row.get("positive_docs")
        negatives = row.get("hard_negatives")
        if not isinstance(positives, list) or not isinstance(negatives, list):
            fail("{}: positive_docs and hard_negatives must be lists".format(context))
        positive_ids: Set[str] = set()
        for collection_name, values in (("positive_docs", positives), ("hard_negatives", negatives)):
            for item in values:
                doc_id = item.get("id") if isinstance(item, dict) else item
                if not isinstance(doc_id, str):
                    fail("{}: entries in {} must be document ids or objects with an id".format(
                        context, collection_name))
                if doc_id not in corpus:
                    fail("{}: unknown corpus document '{}' in {}".format(context, doc_id, collection_name))
                if collection_name == "positive_docs":
                    positive_ids.add(doc_id)
        relations[query_id] = positive_ids
    for query in query_by_id.values():
        if query["type"] == query_type and query["id"] not in relations:
            fail("{}: no relation row for query '{}'".format(path, query["id"]))
    return relations, path


def select_types(value: str) -> List[str]:
    return list(QUERY_TYPES) if value == "all" else [value]


def select_queries(queries: List[Dict[str, str]], query_type: str, limit: Optional[int] = None) -> List[Dict[str, str]]:
    selected = [query for query in queries if query_type == "all" or query["type"] == query_type]
    return selected[:limit] if limit is not None else selected


def source_aliases(query: Dict[str, str], corpus: Dict[str, Dict[str, str]],
                   titles: Dict[str, List[str]]) -> Set[str]:
    ids = {query["paper_id"]} if query["paper_id"] else set()
    source_titles = {query["paper_title"]} if query["paper_title"] else set()
    if query["paper_id"] in corpus and corpus[query["paper_id"]]["title"]:
        source_titles.add(corpus[query["paper_id"]]["title"])
    for title in source_titles:
        key = normalize_title(title)
        if len(key.split()) >= 4:
            ids.update(titles.get(key, []))
    return ids


def title_index(corpus: Dict[str, Dict[str, str]]) -> Dict[str, List[str]]:
    index: Dict[str, List[str]] = {}
    for doc_id, metadata in corpus.items():
        index.setdefault(normalize_title(metadata["title"]), []).append(doc_id)
    return index


def task_record(query: Dict[str, str]) -> Dict[str, str]:
    opaque_id = "task_" + hashlib.sha256(query["id"].encode("utf-8")).hexdigest()[:24]
    return {"id": opaque_id, "type": query["type"], "question": query["question"],
            "cutoff": query["cutoff"]}


def write_jsonl(path: Path, rows: Sequence[Dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as output:
        for row in rows:
            output.write(json.dumps(row, ensure_ascii=False) + "\n")


def prepare(args: argparse.Namespace) -> Dict[str, Any]:
    bench_dir = args.bench_dir
    corpus_path = bench_dir / "corpus.jsonl"
    queries_path = bench_dir / "queries.jsonl"
    corpus = load_corpus(corpus_path)
    queries, query_by_id = load_queries(queries_path)
    selected = select_queries(queries, args.query_type, args.limit)
    selected_types = list(QUERY_TYPES) if args.query_type == "all" else [args.query_type]
    rel_paths = []
    for query_type in selected_types:
        _, rel_path = load_relations(bench_dir, query_type, query_by_id, corpus)
        rel_paths.append(rel_path)
    tasks = [task_record(query) for query in selected]
    write_jsonl(args.output, tasks)
    return {
        "schema_version": SCHEMA_VERSION,
        "protocol": "offline_replay",
        "task_count": len(tasks),
        "query_type": args.query_type,
        "inputs_sha256": {"corpus": sha256(corpus_path), "queries": sha256(queries_path),
                          "relations": {path.name: sha256(path) for path in rel_paths}},
        "tasks_file": str(args.output),
    }


def load_task_selection(path: Path, query_by_id: Dict[str, Dict[str, str]], query_type: str) -> List[str]:
    selected: List[str] = []
    seen: Set[str] = set()
    task_ids = {task_record(query)["id"]: query["id"] for query in query_by_id.values()}
    allowed = {"id", "type", "question", "cutoff"}
    for line_number, row in read_jsonl(path):
        context = "{}:{}".format(path, line_number)
        if set(row) != allowed:
            fail("{}: task rows must contain only id, type, question, and cutoff".format(context))
        query_id = require_string(row, "id", context)
        task_type = require_string(row, "type", context)
        canonical_id = task_ids.get(query_id)
        if canonical_id is None:
            fail("{}: unknown task id '{}'".format(context, query_id))
        query = query_by_id[canonical_id]
        expected = task_record(query)
        if row != expected:
            fail("{}: task '{}' does not match the benchmark query".format(context, query_id))
        if query_type != "all" and task_type != query_type:
            fail("{}: task '{}' is outside selected query type '{}'".format(context, query_id, query_type))
        if canonical_id in seen:
            fail("{}: duplicate task selection for query '{}'".format(context, canonical_id))
        seen.add(canonical_id)
        selected.append(canonical_id)
    if not selected:
        fail("{}: task selection is empty".format(path))
    return selected


def load_run(path: Path, all_query_ids: Set[str], corpus: Dict[str, Dict[str, str]],
             queries: Dict[str, Dict[str, str]], query_type: str) -> Dict[str, Dict[str, Any]]:
    runs: Dict[str, Dict[str, Any]] = {}
    task_ids = {task_record(query)["id"]: query["id"] for query in queries.values()}
    titles = title_index(corpus)
    seen_query_ids: Set[str] = set()
    for line_number, row in read_jsonl(path):
        context = "{}:{}".format(path, line_number)
        supplied_id = require_string(row, "query_id", context)
        query_id = task_ids.get(supplied_id, supplied_id)
        if query_id in seen_query_ids:
            fail("{}: duplicate run query id '{}'".format(context, supplied_id))
        if query_id not in all_query_ids:
            fail("{}: unknown run query id '{}'".format(context, supplied_id))
        seen_query_ids.add(query_id)
        if query_type != "all" and queries[query_id]["type"] != query_type:
            fail("{}: run query '{}' is outside selected query type '{}'".format(context, query_id, query_type))
        aliases = source_aliases(queries[query_id], corpus, titles)
        ranking = row.get("ranking")
        if not isinstance(ranking, list):
            fail("{}: ranking must be a list".format(context))
        ranking_ids: List[str] = []
        ranking_seen: Set[str] = set()
        for item in ranking:
            if not isinstance(item, dict):
                fail("{}: ranking entries must be objects".format(context))
            doc_id = require_string(item, "doc_id", context)
            if doc_id in ranking_seen:
                fail("{}: duplicate ranked document '{}' for query '{}'".format(context, doc_id, query_id))
            if doc_id not in corpus:
                fail("{}: unknown corpus document '{}' in ranking".format(context, doc_id))
            if doc_id in aliases:
                fail("{}: ranking includes the query's source paper or a known title alias '{}'".format(
                    context, doc_id))
            ranking_seen.add(doc_id)
            ranking_ids.append(doc_id)
        has_seen = "seen" in row
        seen_ids: Set[str] = set()
        if has_seen:
            values = row["seen"]
            if not isinstance(values, list):
                fail("{}: seen must be a list".format(context))
            for doc_id in values:
                if not isinstance(doc_id, str):
                    fail("{}: seen entries must be document ids".format(context))
                if doc_id in seen_ids:
                    fail("{}: duplicate seen document '{}' for query '{}'".format(context, doc_id, query_id))
                if doc_id not in corpus:
                    fail("{}: unknown corpus document '{}' in seen".format(context, doc_id))
                if doc_id in aliases:
                    fail("{}: seen includes the query's source paper or a known title alias '{}'".format(
                        context, doc_id))
                seen_ids.add(doc_id)
            outside = set(ranking_ids) - seen_ids
            if outside:
                fail("{}: ranking contains documents outside seen for query '{}': {}".format(
                    context, query_id, ", ".join(sorted(outside))))
        cutoff = queries[query_id]["cutoff"]
        checked = set(ranking_ids) | seen_ids
        future = [doc_id for doc_id in checked if is_after_cutoff(
            corpus[doc_id]["published"], doc_id, cutoff)]
        if future:
            fail("{}: post-cutoff document for query '{}': {}".format(
                context, query_id, ", ".join(sorted(future))))
        runs[query_id] = {"ranking": ranking_ids, "seen": seen_ids, "has_seen": has_seen}
    return runs


def recall_at_k(ranked: Sequence[str], relevant: Set[str], k: int) -> float:
    if not relevant:
        return 0.0
    return len(set(ranked[:k]) & relevant) / len(relevant)


def ndcg_at_k(ranked: Sequence[str], relevant: Set[str], k: int) -> float:
    if not relevant:
        return 0.0
    dcg = sum(1.0 / math.log2(rank + 2) for rank, doc_id in enumerate(ranked[:k]) if doc_id in relevant)
    ideal = sum(1.0 / math.log2(rank + 2) for rank in range(min(len(relevant), k)))
    return dcg / ideal if ideal else 0.0


def round_metric(value: Optional[float]) -> Optional[float]:
    return None if value is None else round(value, 6)


def evaluate_type(query_type: str, query_ids: List[str], query_by_id: Dict[str, Dict[str, str]],
                  positives: Dict[str, Set[str]], runs: Dict[str, Dict[str, Any]],
                  corpus: Dict[str, Dict[str, str]], titles: Dict[str, List[str]]) -> Dict[str, Any]:
    per_query = []
    recalls: Dict[int, List[float]] = {5: [], 20: [], 100: []}
    ndcgs: List[float] = []
    trajectory: List[float] = []
    depth_counts = {"0": 0, "1-4": 0, "5-19": 0, "20-99": 0, "100+": 0}
    missing = 0
    n_seen = 0
    n_seen_scored = 0
    unknown_cutoffs = 0
    unknown_ranking_dates = 0
    unknown_seen_dates = 0
    source_title_unavailable = 0
    unreachable_positive_count = 0
    queries_with_unreachable_positives = 0
    n_scored = 0
    n_no_positive = 0
    for query_id in query_ids:
        query = query_by_id[query_id]
        if parse_month(query["cutoff"]) is None:
            unknown_cutoffs += 1
        title_available = bool(query["paper_title"] or
                               (query["paper_id"] in corpus and corpus[query["paper_id"]]["title"]))
        if query["paper_id"] and not title_available:
            source_title_unavailable += 1
        record = runs.get(query_id)
        if record is None:
            missing += 1
            ranked: List[str] = []
            depth = 0
        else:
            aliases = source_aliases(query, corpus, titles)
            ranked = [doc_id for doc_id in record["ranking"] if doc_id not in aliases]
            depth = len(ranked)
            unknown_ranking_dates += sum(parse_month(corpus[doc_id]["published"], doc_id) is None
                                         for doc_id in record["ranking"])
            unknown_seen_dates += sum(parse_month(corpus[doc_id]["published"], doc_id) is None
                                      for doc_id in record["seen"])
        depth_counts["0" if depth == 0 else "1-4" if depth < 5 else
                     "5-19" if depth < 20 else "20-99" if depth < 100 else "100+"] += 1
        relevant = positives[query_id]
        if relevant:
            n_scored += 1
        else:
            n_no_positive += 1
        unreachable = {doc_id for doc_id in relevant if is_after_cutoff(
            corpus[doc_id]["published"], doc_id, query["cutoff"])}
        unreachable_positive_count += len(unreachable)
        if unreachable:
            queries_with_unreachable_positives += 1
        attainable_ceiling = len(relevant - unreachable) / len(relevant) if relevant else 0.0
        row_metrics = {"recall@{}".format(k): recall_at_k(ranked, relevant, k) for k in (5, 20, 100)}
        row_metrics["ndcg@20"] = ndcg_at_k(ranked, relevant, 20)
        for k, values in recalls.items():
            values.append(row_metrics["recall@{}".format(k)])
        ndcgs.append(row_metrics["ndcg@20"])
        if record is not None and record["has_seen"]:
            n_seen += 1
            if relevant:
                n_seen_scored += 1
                touched = record["seen"] - source_aliases(query, corpus, titles)
                trajectory.append(len(relevant & touched) / len(relevant))
        per_query.append({"query_id": query_id, "ranking_depth": depth,
            "temporally_unreachable_positive_count": len(unreachable),
            "attainable_recall_ceiling": round_metric(attainable_ceiling), **{
            key: round_metric(value) for key, value in row_metrics.items()}})
    n_queries = len(query_ids)
    return {
        "n_queries": n_queries,
        "n_requested": n_queries,
        "n_scored": n_scored,
        "n_no_positive": n_no_positive,
        "missing_run_queries": missing,
        "ranking_depth_counts": depth_counts,
        "partial_run": missing > 0 or depth_counts["0"] > 0 or depth_counts["1-4"] > 0
                        or depth_counts["5-19"] > 0 or depth_counts["20-99"] > 0,
        "recall@5": round_metric(sum(recalls[5]) / n_scored if n_scored else None),
        "recall@20": round_metric(sum(recalls[20]) / n_scored if n_scored else None),
        "recall@100": round_metric(sum(recalls[100]) / n_scored if n_scored else None),
        "ndcg@20": round_metric(sum(ndcgs) / n_scored if n_scored else None),
        "n_seen": n_seen,
        "n_seen_scored": n_seen_scored,
        "unknown_cutoff_queries": unknown_cutoffs,
        "unknown_ranking_dates": unknown_ranking_dates,
        "unknown_seen_dates": unknown_seen_dates,
        "source_title_unavailable": source_title_unavailable,
        "temporally_unreachable_positive_count": unreachable_positive_count,
        "queries_with_unreachable_positives": queries_with_unreachable_positives,
        "candidate_trajectory_recall": round_metric(sum(trajectory) / len(trajectory) if trajectory else None),
        "per_query": per_query,
    }


def evaluate(args: argparse.Namespace) -> Dict[str, Any]:
    bench_dir = args.bench_dir
    corpus_path = bench_dir / "corpus.jsonl"
    queries_path = bench_dir / "queries.jsonl"
    corpus = load_corpus(corpus_path)
    all_queries, query_by_id = load_queries(queries_path)
    selected = ([query_by_id[query_id] for query_id in load_task_selection(
        args.query_ids, query_by_id, args.query_type)] if args.query_ids else
        select_queries(all_queries, args.query_type))
    if not selected:
        fail("the selected query cohort is empty")
    selected_ids = [query["id"] for query in selected]
    selected_types = list(QUERY_TYPES) if args.query_type == "all" and not args.query_ids else sorted(
        {query["type"] for query in selected})
    positives: Dict[str, Set[str]] = {}
    rel_paths = []
    for query_type in selected_types:
        relation_rows, rel_path = load_relations(bench_dir, query_type, query_by_id, corpus)
        positives.update(relation_rows)
        rel_paths.append(rel_path)
    runs = load_run(args.run, set(query_by_id), corpus, query_by_id, args.query_type)
    titles = title_index(corpus)
    by_type = {}
    for query_type in selected_types:
        ids = [query_id for query_id in selected_ids if query_by_id[query_id]["type"] == query_type]
        by_type[query_type] = evaluate_type(query_type, ids, query_by_id, positives, runs, corpus, titles)
    return {
        "schema_version": SCHEMA_VERSION,
        "protocol": "offline_replay",
        "query_cohort": {"query_type": args.query_type, "query_ids": selected_ids},
        "counts": {"corpus": len(corpus), "queries": len(selected_ids),
                   "run_rows": len(runs), "missing_run_rows": sum(v["missing_run_queries"] for v in by_type.values()),
                   "n_seen": sum(v["n_seen"] for v in by_type.values())},
        "inputs_sha256": {"corpus": sha256(corpus_path), "queries": sha256(queries_path),
                          "relations": {path.name: sha256(path) for path in rel_paths},
                          "run": sha256(args.run)},
        "metrics": by_type,
    }


def positive_limit(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError("limit must be a positive integer")
    if parsed <= 0:
        raise argparse.ArgumentTypeError("limit must be a positive integer")
    return parsed


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare", help="write gold-free task JSONL")
    prepare_parser.add_argument("--bench-dir", type=Path, required=True)
    prepare_parser.add_argument("--output", type=Path, required=True)
    prepare_parser.add_argument("--limit", type=positive_limit)
    prepare_parser.add_argument("--query-type", choices=("all",) + QUERY_TYPES, default="all")
    eval_parser = commands.add_parser("evaluate", help="score an offline JSONL run")
    eval_parser.add_argument("--bench-dir", type=Path, required=True)
    eval_parser.add_argument("--run", type=Path, required=True)
    eval_parser.add_argument("--query-type", choices=("all",) + QUERY_TYPES, default="all")
    eval_parser.add_argument("--query-ids", type=Path, help="gold-free task JSONL from prepare")
    eval_parser.add_argument("--output", type=Path, required=True)
    return root


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = parser().parse_args(argv)
    try:
        input_paths = [args.bench_dir / "corpus.jsonl", args.bench_dir / "queries.jsonl"]
        input_paths.extend(args.bench_dir / "rels" / TYPE_FILES[item] for item in QUERY_TYPES)
        if args.command == "evaluate":
            input_paths.append(args.run)
            if args.query_ids:
                input_paths.append(args.query_ids)
        ensure_output_distinct(args.output, input_paths)
        if args.command == "prepare":
            report = prepare(args)
            print(json.dumps(report, indent=2, ensure_ascii=False))
        else:
            report = evaluate(args)
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    except (EvaluationError, OSError) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
