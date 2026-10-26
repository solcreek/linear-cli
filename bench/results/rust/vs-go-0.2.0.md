- **go-0.2.0-release**: linearctl 0.2.0 at `a1334d1`, lawrences-macbook-air, Darwin arm64, Apple M3, load 3.86 4.38 4.66, 300 runs, 2026-10-26T17:51:46Z
- **rust**: linearctl 0.2.0 at `a1334d1`, lawrences-macbook-air, Darwin arm64, Apple M3, load 4.83 4.60 4.71, 300 runs, 2026-10-26T17:53:36Z

| scenario | go-0.2.0-release median | rust median | speed-up | go-0.2.0-release RSS | rust RSS |
|---|---:|---:|---:|---:|---:|
| version | 4.25 ms | 2.69 ms | 1.58× | 13.8 MiB | 6.6 MiB |
| help | 4.31 ms | 2.75 ms | 1.57× | 14.0 MiB | 6.8 MiB |
| issue-list-help | 4.42 ms | 2.83 ms | 1.56× | 13.7 MiB | 6.9 MiB |
| describe | 5.34 ms | 3.49 ms | 1.53× | 15.4 MiB | 8.0 MiB |
| issue-list | 6.69 ms | 3.96 ms | 1.69× | 17.5 MiB | 8.6 MiB |
| issue-show | 5.52 ms | 3.47 ms | 1.59× | 16.0 MiB | 7.9 MiB |
| demo-issue-list | 5.31 ms | 3.41 ms | 1.56× | 14.9 MiB | 7.9 MiB |
| serve-100 | 270.43 ms | 132.80 ms | 2.04× | 21.4 MiB | 9.8 MiB |
| binary size | 9.15 MiB | 2.47 MiB | 3.70× smaller | | |
