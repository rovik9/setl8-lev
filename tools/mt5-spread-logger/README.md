# MT5 spread logger (decision 14 replacement: copy Exness)

Read-only MQL5 script. Samples bid and ask of the 20 launch symbols every 5 s from an Exness MT5 terminal and writes CSV files
to the MT5 `Common\Files` folder. It never trades and never touches account data. NOT compiled or tested by the author (no MT5 in the
build container): compile it first (MetaEditor, F7); fix any error it reports and tell me.

## Run
1. In the Exness MT5 terminal open MetaEditor (F4), then -> File -> New -> Script -> paste `SpreadLogger.mq5` -> Compile.
2. Check the symbol names in the script inputs against Market Watch (suffix depends on account type). Set `InpTag` to the account type
   (`exness_standard`, `exness_raw`, ...). Use ONE account type; the one whose spreads and commissions we copy.
3. Drag the script onto any chart. It runs until you remove it. Keep MT5 running and online: Sunday 2026-10-18 21:00 UTC through
   Monday 2026-10-19 21:30 UTC (the same window as the session observer) is ideal, 24 h minimum including a US cash-open hour.
4. Files: `<Common>\Files\setl8_<tag>_samples_<stamp>.csv` and `..._spec_<stamp>.csv` (contract size, digits, tick size).
5. ALSO write down, from the Exness site or the terminal's symbol specification, the COMMISSION per lot per side for that account
   type, per instrument group (crypto, metals, energies, indices, stocks). It is not available to the script.

The raw CSVs are not committed. A later prompt converts them: half-spread e6 = (ask - bid) / 2 / mid x 1e6 per market, taken from the
in-session samples only, and commission e6 = commission per side / (contract size x price) x 1e6 per class.
