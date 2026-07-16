# Benchmark Policy

Benchmark dishonesty is the original sin of this ecosystem. One famous framework reached 8.5k stars on a pipelining-rigged "1M req/s" claim and died under the takedown; another launched with benchmarks comparing its multi-core runtime against single-core competitor setups and paid a credibility tax for years. Rayo's benchmarks are engineered to be *believed*, which means engineered to be attacked and survive.

## Rules

1. **Reproducible or it doesn't exist.** Every published number comes from a Dockerized harness in `benchmarks/` that anyone can run with one command. Hardware, versions, and configuration are recorded in the output.
2. **Same resources for every contender.** Same core count, same memory limits, same connection counts. If a framework needs N workers to use N cores, it gets N workers, properly configured (uvicorn with httptools and uvloop, gunicorn tuned — the *best* configuration of the competitor, not the default).
3. **Realistic workloads first.** The headline suite is: JSON body validation + a database query + serialized response. Hello-world plaintext exists in the suite but is never a headline.
4. **Publish losses.** The results page always includes configurations where Rayo is not the fastest (e.g., DB-bound endpoints where the framework layer is noise). If we have no losses to show, our suite is too narrow — that's a bug.
5. **Report distributions, not averages.** p50/p90/p99/max, plus memory (RSS) and cold-start time. Tail latency and memory-per-core are Rayo's actual story; averages hide what matters.
6. **No cross-hardware comparisons, no numbers from other projects' READMEs.** We only publish numbers our harness produced.
7. **Claims are versioned.** Every published result links the harness commit that produced it. When a competitor improves, we re-run and update — including when it makes us look worse.

## What we benchmark

| Suite | What it measures |
|---|---|
| `validate-db-serialize` | The headline: realistic API request lifecycle |
| `cold-start` | Import → first request, at 10/100/1000 routes |
| `streaming` | SSE per-token latency and backpressure under slow clients |
| `blocking-abuse` | p99 under a misbehaving (blocking) handler — the resilience story |
| `free-threaded` | Cores vs throughput on 3.14t, one process, shared pool |
| `plaintext` | Hello world (in the suite for comparability; never a headline) |

## Why this is strategy, not just hygiene

TechEmpower is gone (final round March 2026). The teams that adopted Rust servers in production (Sentry, GlitchTip, Talk Python) cite operational qualities, not leaderboard positions. Honest, reproducible, distribution-focused benchmarks are now a *differentiator* — the marketing plan depends on this policy holding.
