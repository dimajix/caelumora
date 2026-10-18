#!/bin/sh
# The runs of 2026-10-18 (issue 424), one after the other: they share one GPU.
set -e
cd "$(dirname "$0")"
python3 experiment.py probes --runs 5 > results/probes.log 2>&1
python3 experiment.py now --runs 5 > results/now.log 2>&1
python3 experiment.py imitation --formats xml --runs 2 > results/imitation-xml.log 2>&1
python3 experiment.py imitation --formats prefix --runs 1 > results/imitation-prefix.log 2>&1
# Added after the first run: the messages carried the date, which the model
# took for today. The current time without times on the messages, and a
# question answered two days after it was asked.
python3 experiment.py now --runs 5 --message-style none --tag=-untimed > results/now-untimed.log 2>&1
python3 experiment.py now --runs 5 --probes late_open_now --tag=-late > results/now-late.log 2>&1
# The probes in a German channel, with the time written in English and in German.
python3 experiment.py probes --lang de --runs 5 --styles none iso long_en long_de --tag=-de > results/probes-de.log 2>&1
