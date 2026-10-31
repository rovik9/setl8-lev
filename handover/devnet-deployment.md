# Devnet deployment of the lev trading challenge

Public data only. Machine-readable twin: `handover/devnet-deployment.json`.

**The admin keys of this deployment are throwaway keys of a rehearsal. They must never be reused anywhere.**

The core vault holds a product registry; one product is registered: lev trading (the leveraged-trading challenge).

## Cluster

- cluster: `devnet`
- genesis hash: `EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG`

## Programs

- vault program: [`qmeDsSeK7eFSU8aNoqLFJxZstoMA3EgMrcsuMFjsF2Z`](https://explorer.solana.com/address/qmeDsSeK7eFSU8aNoqLFJxZstoMA3EgMrcsuMFjsF2Z?cluster=devnet)
- vault sha256: `ef6bbe719c466006795c43eb1cf708a9e1fec42c92b4e29e7c66a9bf61a0f15c`
- vault size (bytes): `535712`
- lev trading program: [`HmkvQfgCMnihGzdZE1FTKVacZN76JS9bfEu2QBJBAdvY`](https://explorer.solana.com/address/HmkvQfgCMnihGzdZE1FTKVacZN76JS9bfEu2QBJBAdvY?cluster=devnet)
- lev sha256: `03b0333240a531af718f297d803d512efaf2cb5cd1edf21712e0734e179343ee`
- lev size (bytes): `862704`
- derived from the localnet build (sha256): `43fdd9356c774ea343686e5f6bf781b998a01670664c2af6b8233bf97f22bc1b`

## The vault

- vault state: [`G7qpP8L2kXGP7VYuvhioEQLEuWxUngxDmxqxEqnpUCbT`](https://explorer.solana.com/address/G7qpP8L2kXGP7VYuvhioEQLEuWxUngxDmxqxEqnpUCbT?cluster=devnet)
- product registry: [`FUVukP8gz8kpxXXt3UbE4C3UCAcp31gRMqEacQvUP9pv`](https://explorer.solana.com/address/FUVukP8gz8kpxXXt3UbE4C3UCAcp31gRMqEacQvUP9pv?cluster=devnet)
- payout tally: [`8MNMVmXC2zDyrfuy2ABAya4mrNHFsCeXNhf6VQ8paaBK`](https://explorer.solana.com/address/8MNMVmXC2zDyrfuy2ABAya4mrNHFsCeXNhf6VQ8paaBK?cluster=devnet)
- USDC payout pool: [`UjqJZRCALei1whzfLJbX3J5p22hRyYrVF7v3xNPiKPi`](https://explorer.solana.com/address/UjqJZRCALei1whzfLJbX3J5p22hRyYrVF7v3xNPiKPi?cluster=devnet)
- USDT payout pool: [`HEhuquTgApHjVcAxtJ1HoURfru9WU8T9fUqhVGTRsuvF`](https://explorer.solana.com/address/HEhuquTgApHjVcAxtJ1HoURfru9WU8T9fUqhVGTRsuvF?cluster=devnet)

## Lev trading

- config: [`BySmg4NH3BJfBgZcPFSGJP3acRmqySGE2LvNk8GUF8xn`](https://explorer.solana.com/address/BySmg4NH3BJfBgZcPFSGJP3acRmqySGE2LvNk8GUF8xn?cluster=devnet)
- rule set: [`NFByy5LbVEvHCuEcuC1WVpZ5ModpNQb4nEXYdNhBTmj`](https://explorer.solana.com/address/NFByy5LbVEvHCuEcuC1WVpZ5ModpNQb4nEXYdNhBTmj?cluster=devnet)
- event authority: [`6VHXUR9bt5VL9kx913utojQXXsmthiiWj3T1USvjQCZy`](https://explorer.solana.com/address/6VHXUR9bt5VL9kx913utojQXXsmthiiWj3T1USvjQCZy?cluster=devnet)
- program authority: [`4RFsP8wDuZee6LbaSjShA8eqgt8oEfcRUW1bQKCgjSKe`](https://explorer.solana.com/address/4RFsP8wDuZee6LbaSjShA8eqgt8oEfcRUW1bQKCgjSKe?cluster=devnet)
- pass delay (seconds): `86400`
- challenge fee (bps of the price): `40`

### Tiers

| tier | account size (USD) | price (USD) |
|---|---:|---:|
| 0 | 2500 | 30 |
| 1 | 5000 | 55 |
| 2 | 10000 | 99 |
| 3 | 15000 | 145 |
| 4 | 20000 | 189 |
| 5 | 25000 | 235 |
| 6 | 50000 | 489 |
| 7 | 75000 | 689 |
| 8 | 100000 | 859 |
| 9 | 200000 | 1599 |

### Markets (21)

| id | symbol | address |
|---:|---|---|
| 0 | BTC | [`8VeHbnGd9jShAcj6MuxAWj1jrKZDKAnTTjNyU2138YNg`](https://explorer.solana.com/address/8VeHbnGd9jShAcj6MuxAWj1jrKZDKAnTTjNyU2138YNg?cluster=devnet) |
| 1 | ETH | [`3q5MQJMRwCMAE4KaLojZ1nWBFQnq6VJYUmRzP68XSxf2`](https://explorer.solana.com/address/3q5MQJMRwCMAE4KaLojZ1nWBFQnq6VJYUmRzP68XSxf2?cluster=devnet) |
| 2 | SOL | [`8rnkqBa87twdKZ7spwzWUF1NaDL7RYJhnntaFdieWZyD`](https://explorer.solana.com/address/8rnkqBa87twdKZ7spwzWUF1NaDL7RYJhnntaFdieWZyD?cluster=devnet) |
| 3 | GOLD | [`EkugSYv2wLWbXaMVgAWbpHMK4jcFEYaZJCWwQAxvoBT1`](https://explorer.solana.com/address/EkugSYv2wLWbXaMVgAWbpHMK4jcFEYaZJCWwQAxvoBT1?cluster=devnet) |
| 4 | SILVER | [`5rf8mAch1znDv7csmsiw4yidYmQzdhHsGT4QSMXbqjQN`](https://explorer.solana.com/address/5rf8mAch1znDv7csmsiw4yidYmQzdhHsGT4QSMXbqjQN?cluster=devnet) |
| 5 | WTIOIL | [`AGxPViza3FbnwWmMohhuaABKrf7FzJEfEwvzdUkXFcsc`](https://explorer.solana.com/address/AGxPViza3FbnwWmMohhuaABKrf7FzJEfEwvzdUkXFcsc?cluster=devnet) |
| 6 | BRENTOIL | [`Gdzy6EVLoXyri6mTs6YtFqqqDhj2qn5Qe7XAkhdqn2dE`](https://explorer.solana.com/address/Gdzy6EVLoXyri6mTs6YtFqqqDhj2qn5Qe7XAkhdqn2dE?cluster=devnet) |
| 7 | SPY | [`HWiRpqAQYE4WdZcxgVmU948mYRBRxFZts6Q9w6B6LB7e`](https://explorer.solana.com/address/HWiRpqAQYE4WdZcxgVmU948mYRBRxFZts6Q9w6B6LB7e?cluster=devnet) |
| 8 | QQQ | [`DPWD7Tv6GZ876kEsjppABcBmLCRNUnMoMk3wd2QFgy2K`](https://explorer.solana.com/address/DPWD7Tv6GZ876kEsjppABcBmLCRNUnMoMk3wd2QFgy2K?cluster=devnet) |
| 9 | xyz:JP225 | [`3uLjVvMJi9aNGL4SKuRUpo5C5aTTSmzbYYXbyvhnww35`](https://explorer.solana.com/address/3uLjVvMJi9aNGL4SKuRUpo5C5aTTSmzbYYXbyvhnww35?cluster=devnet) |
| 10 | xyz:KR200 | [`CQUUTU3Ao5RWXhJNpWxqBG2FkiW3fxeAzF8pCGrUUPRx`](https://explorer.solana.com/address/CQUUTU3Ao5RWXhJNpWxqBG2FkiW3fxeAzF8pCGrUUPRx?cluster=devnet) |
| 11 | xyz:AAPL | [`6VrDPvUjEFNjMS2w5naDSVhUZ9hp145nkT6FKapNQd1c`](https://explorer.solana.com/address/6VrDPvUjEFNjMS2w5naDSVhUZ9hp145nkT6FKapNQd1c?cluster=devnet) |
| 12 | xyz:AMD | [`EFifmVQJWF6QsKXDcUHsKaypA1TUWbC5P5PqaXdwDTnY`](https://explorer.solana.com/address/EFifmVQJWF6QsKXDcUHsKaypA1TUWbC5P5PqaXdwDTnY?cluster=devnet) |
| 13 | xyz:AMZN | [`HQGRqdhsCkYQb9Z1BBStCCzPD3J4qidwmk4qb68Ax9fc`](https://explorer.solana.com/address/HQGRqdhsCkYQb9Z1BBStCCzPD3J4qidwmk4qb68Ax9fc?cluster=devnet) |
| 14 | xyz:GOOGL | [`Cp8HUQbGuKJ7azeBaGvJdZ4GwG6DYrGwb1oSFJFkqEuV`](https://explorer.solana.com/address/Cp8HUQbGuKJ7azeBaGvJdZ4GwG6DYrGwb1oSFJFkqEuV?cluster=devnet) |
| 15 | xyz:META | [`TtE2HxLyUKMveU2LEPCqB5JgWWmYaLwirhUtBVbLwvX`](https://explorer.solana.com/address/TtE2HxLyUKMveU2LEPCqB5JgWWmYaLwirhUtBVbLwvX?cluster=devnet) |
| 16 | xyz:MSFT | [`BmcgUfsxPQQCybWhjoDwg4dA2NbhWQKZGkaNmP78vp6y`](https://explorer.solana.com/address/BmcgUfsxPQQCybWhjoDwg4dA2NbhWQKZGkaNmP78vp6y?cluster=devnet) |
| 17 | xyz:NFLX | [`EQvrAyoe9FRjFe5xmfW7UrSJ6UWEdvFkewYHh4qZtUQh`](https://explorer.solana.com/address/EQvrAyoe9FRjFe5xmfW7UrSJ6UWEdvFkewYHh4qZtUQh?cluster=devnet) |
| 18 | xyz:NVDA | [`C1CNTJBXwdGr6L7mGV8oWkiHwMbQPXC4Yrimf9tQZxBn`](https://explorer.solana.com/address/C1CNTJBXwdGr6L7mGV8oWkiHwMbQPXC4Yrimf9tQZxBn?cluster=devnet) |
| 19 | xyz:PLTR | [`ACUeLGdQtriZ1agUoJUokhxWWLzLtomCA6SjSsKGXdcz`](https://explorer.solana.com/address/ACUeLGdQtriZ1agUoJUokhxWWLzLtomCA6SjSsKGXdcz?cluster=devnet) |
| 20 | xyz:TSLA | [`GURS3oX7DcCo1D9EsQ15b1a1bvxokfiGWFYrQUUxVQuw`](https://explorer.solana.com/address/GURS3oX7DcCo1D9EsQ15b1a1bvxokfiGWFYrQUUxVQuw?cluster=devnet) |

## Test mints and keys

- test USDC mint: [`2kBKxj3TYz2UbQfVcJ5xBbWscC5hrALq7MLzJi3c1fTx`](https://explorer.solana.com/address/2kBKxj3TYz2UbQfVcJ5xBbWscC5hrALq7MLzJi3c1fTx?cluster=devnet)
- test USDT mint: [`9qfVj2AgFpWVM4jowh2nNkQHcjKECPhJnWbkW9y71BVo`](https://explorer.solana.com/address/9qfVj2AgFpWVM4jowh2nNkQHcjKECPhJnWbkW9y71BVo?cluster=devnet)
- fee wallet (also the test mint authority): [`98j1TXdxt77vsDVcLmABWyMP2bWXVoYqRg85nGVGvcth`](https://explorer.solana.com/address/98j1TXdxt77vsDVcLmABWyMP2bWXVoYqRg85nGVGvcth?cluster=devnet)
- evaluator public key: [`9kSc8VXbUgRKwxcVaqBPnraZzE5eLT2cGP8bcqy7Kpb7`](https://explorer.solana.com/address/9kSc8VXbUgRKwxcVaqBPnraZzE5eLT2cGP8bcqy7Kpb7?cluster=devnet)
- evaluator key file (local devnet use only; the file is not part of this manifest): `~/.setl8-devnet-lev/evaluator.json`
- vault admin SL8 (throwaway): [`HiB78C1x7TMsHzkAzvxwaVGvkFCPFdxy4WzcV2NmE9i2`](https://explorer.solana.com/address/HiB78C1x7TMsHzkAzvxwaVGvkFCPFdxy4WzcV2NmE9i2?cluster=devnet)
- vault admin ROV (throwaway): [`2hdsCzasnWDo96STaZYNWaFUoHWt5qLQGrMTwGqU5HtH`](https://explorer.solana.com/address/2hdsCzasnWDo96STaZYNWaFUoHWt5qLQGrMTwGqU5HtH?cluster=devnet)
- lev admin SL8 (throwaway): [`AbJ7iwVVrZHjkVgHmuKgUVdYW3qVRjLZNKnKECYR6Shf`](https://explorer.solana.com/address/AbJ7iwVVrZHjkVgHmuKgUVdYW3qVRjLZNKnKECYR6Shf?cluster=devnet)
- lev admin ROV (throwaway): [`BFbW5FYZTkUcJdxcxCDW4XM9rz4kgb4VatYEKR1bfitJ`](https://explorer.solana.com/address/BFbW5FYZTkUcJdxcxCDW4XM9rz4kgb4VatYEKR1bfitJ?cluster=devnet)
- upgrade authority (throwaway): [`GxDJcsyKDk7iS3eAhDGigsTWmuBvzY5z8WkNhVPP2a9p`](https://explorer.solana.com/address/GxDJcsyKDk7iS3eAhDGigsTWmuBvzY5z8WkNhVPP2a9p?cluster=devnet)
- address lookup table: none was created by this run

## Example accounts

| name | role | trading account | trader wallet |
|---|---|---|---|
| A1 | honest challenge: passes phase 1 and phase 2, the pass is finalized after the delay and the payout is paid | [`A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU`](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet) | [`7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF`](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet) |
| B1 | fails the static floor in phase 1; reset (ladder step 1) opens B2 | [`cNZUjvHMLoMacyyDjXSMvg5m8ZoEhvuPy8r3bRnNdze`](https://explorer.solana.com/address/cNZUjvHMLoMacyyDjXSMvg5m8ZoEhvuPy8r3bRnNdze?cluster=devnet) | [`748HMeryS2sPS91vE5QNwfFNxWp8X7aHd1EK9w5sqjPB`](https://explorer.solana.com/address/748HMeryS2sPS91vE5QNwfFNxWp8X7aHd1EK9w5sqjPB?cluster=devnet) |
| B2 | passes phase 1, fails in phase 2; reset (ladder step 2) opens B3 | [`bJ6Dd7RoBcNAt8LBh3JsJV8iuBSAMTV8GUW9YvuACwh`](https://explorer.solana.com/address/bJ6Dd7RoBcNAt8LBh3JsJV8iuBSAMTV8GUW9YvuACwh?cluster=devnet) | [`748HMeryS2sPS91vE5QNwfFNxWp8X7aHd1EK9w5sqjPB`](https://explorer.solana.com/address/748HMeryS2sPS91vE5QNwfFNxWp8X7aHd1EK9w5sqjPB?cluster=devnet) |
| B3 | the account opened by the second reset, in phase 2 | [`3EVjgKZzirtGWTYJxZcqx97z4NNTjEhLBpjKjgiQhZHn`](https://explorer.solana.com/address/3EVjgKZzirtGWTYJxZcqx97z4NNTjEhLBpjKjgiQhZHn?cluster=devnet) | [`748HMeryS2sPS91vE5QNwfFNxWp8X7aHd1EK9w5sqjPB`](https://explorer.solana.com/address/748HMeryS2sPS91vE5QNwfFNxWp8X7aHd1EK9w5sqjPB?cluster=devnet) |
| C1 | a hold and then a failure while held (review, then no reset) | [`CTwfWjHq8h4rWz2JtwTZdRyW417cuzWynbtRPXnVuZJ4`](https://explorer.solana.com/address/CTwfWjHq8h4rWz2JtwTZdRyW417cuzWynbtRPXnVuZJ4?cluster=devnet) | [`hrCZYN74Qg4LKraXUgwz8dB18A4j7LasJkp1GJLWHLx`](https://explorer.solana.com/address/hrCZYN74Qg4LKraXUgwz8dB18A4j7LasJkp1GJLWHLx?cluster=devnet) |
| D1 | a confirmed phase 2 pass that both admins veto | [`6nUiF9Xza78cn6x3UermVTfNYeyS4Y1itpw7DJW54PHV`](https://explorer.solana.com/address/6nUiF9Xza78cn6x3UermVTfNYeyS4Y1itpw7DJW54PHV?cluster=devnet) | [`9ic6XQamxpQrJTi6yevbPbF4s4kdstdvWTYm6LQjgTyP`](https://explorer.solana.com/address/9ic6XQamxpQrJTi6yevbPbF4s4kdstdvWTYm6LQjgTyP?cluster=devnet) |
| E1 | the forced close-all after a quick-limit overshoot | [`FKUpNp5enNSCoCCwSLRjWNCinhrW4SUGi2vJUBTUWzdB`](https://explorer.solana.com/address/FKUpNp5enNSCoCCwSLRjWNCinhrW4SUGi2vJUBTUWzdB?cluster=devnet) | [`9GJb6mKGFGBfQwGEwmbz8cALbGNoeWeDAkwndnBnG681`](https://explorer.solana.com/address/9GJb6mKGFGBfQwGEwmbz8cALbGNoeWeDAkwndnBnG681?cluster=devnet) |
| F1 | a breach with the 12 hour lock (rehearsal price proofs) | [`jogP7GpWChMkfAiDxzcafXtPnXBoGHkfmFXgPd6nKKL`](https://explorer.solana.com/address/jogP7GpWChMkfAiDxzcafXtPnXBoGHkfmFXgPd6nKKL?cluster=devnet) | [`DtjS6Jq58h3mfGBFP2pUoNGaTfLVysPkMFKjBWvpDuT5`](https://explorer.solana.com/address/DtjS6Jq58h3mfGBFP2pUoNGaTfLVysPkMFKjBWvpDuT5?cluster=devnet) |
| G1 | helper account of the refusal and measurement checks (rehearsal price proofs) | [`CnBivPHhFAvr6r5DXxihvBrZCHQwphp5fv5GPSwjK64u`](https://explorer.solana.com/address/CnBivPHhFAvr6r5DXxihvBrZCHQwphp5fv5GPSwjK64u?cluster=devnet) | [`3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq`](https://explorer.solana.com/address/3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq?cluster=devnet) |
| G2 | helper account of the refusal and measurement checks (rehearsal price proofs) | [`47aY7smgCPdQ3yx7FRPFMp8Fny7jNDYYUrXiQfZ9MR4R`](https://explorer.solana.com/address/47aY7smgCPdQ3yx7FRPFMp8Fny7jNDYYUrXiQfZ9MR4R?cluster=devnet) | [`3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq`](https://explorer.solana.com/address/3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq?cluster=devnet) |
| H1 | helper account of the refusal and measurement checks (rehearsal price proofs) | [`5t9yJio8J2kuvDHMPCadZTvpLYNdjUbyhAb565QbiGcq`](https://explorer.solana.com/address/5t9yJio8J2kuvDHMPCadZTvpLYNdjUbyhAb565QbiGcq?cluster=devnet) | [`3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq`](https://explorer.solana.com/address/3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq?cluster=devnet) |
| H2 | helper account of the refusal and measurement checks (rehearsal price proofs) | [`7VJcdmBiMNXn3uSc9BeEgMo6D6AZxgsdhESW4UvpYLDz`](https://explorer.solana.com/address/7VJcdmBiMNXn3uSc9BeEgMo6D6AZxgsdhESW4UvpYLDz?cluster=devnet) | [`3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq`](https://explorer.solana.com/address/3MZeT3DxtzEcHD87hF9HTCY5z33odDzBHFDGPBBS3pWq?cluster=devnet) |
| H3 | helper account of the refusal and measurement checks (rehearsal price proofs) | [`4vGSmZM2SuhTJcGaoEce8DTVmE4s3cxR1LQQoXtFrkA3`](https://explorer.solana.com/address/4vGSmZM2SuhTJcGaoEce8DTVmE4s3cxR1LQQoXtFrkA3?cluster=devnet) | [`9GJb6mKGFGBfQwGEwmbz8cALbGNoeWeDAkwndnBnG681`](https://explorer.solana.com/address/9GJb6mKGFGBfQwGEwmbz8cALbGNoeWeDAkwndnBnG681?cluster=devnet) |
| H4 | helper account of the refusal and measurement checks (rehearsal price proofs) | [`Ca9VpxBe4JfFQvWixmw5JW9cAYBWF68eveNxVvaRHie7`](https://explorer.solana.com/address/Ca9VpxBe4JfFQvWixmw5JW9cAYBWF68eveNxVvaRHie7?cluster=devnet) | [`9GJb6mKGFGBfQwGEwmbz8cALbGNoeWeDAkwndnBnG681`](https://explorer.solana.com/address/9GJb6mKGFGBfQwGEwmbz8cALbGNoeWeDAkwndnBnG681?cluster=devnet) |

## One example transaction per instruction

- `confirm_pass`: [U7H8c65YTkgjhMYfkd1AB7bCfCmD7m2jC7Se2pXEPQ18vCghYDZUczLKZvRJQfPuKvKTJZ45YrZ2wSoYGzR8Rvk](https://explorer.solana.com/tx/U7H8c65YTkgjhMYfkd1AB7bCfCmD7m2jC7Se2pXEPQ18vCghYDZUczLKZvRJQfPuKvKTJZ45YrZ2wSoYGzR8Rvk?cluster=devnet)
- `create_ata`: [21R4pY5KLPwyCemZ9mM8cYfGf1E8ProtUPqYnDM6pbnsn1Z2hHqhK66vKuUMMmdAhmZRsJikwWpW37jPcbaY3Zp8](https://explorer.solana.com/tx/21R4pY5KLPwyCemZ9mM8cYfGf1E8ProtUPqYnDM6pbnsn1Z2hHqhK66vKuUMMmdAhmZRsJikwWpW37jPcbaY3Zp8?cluster=devnet)
- `create_market`: [fxEZLqUCF3fQEjgsjKveHBkNzcjATxHWQDGSPaLBnKRxV5J8Mns65ymc9VHUs3FsSMNFzeBdghs62MrpeEokvvZ](https://explorer.solana.com/tx/fxEZLqUCF3fQEjgsjKveHBkNzcjATxHWQDGSPaLBnKRxV5J8Mns65ymc9VHUs3FsSMNFzeBdghs62MrpeEokvvZ?cluster=devnet)
- `create_mint`: [3YZuKSENoBonFTcAEUojJXxbmB1e9B59AfVSAgbSZxxAi4rL4VY3pSRDrsa9h36Vf8Q3QW4voutxAcaw2sMNmAqx](https://explorer.solana.com/tx/3YZuKSENoBonFTcAEUojJXxbmB1e9B59AfVSAgbSZxxAi4rL4VY3pSRDrsa9h36Vf8Q3QW4voutxAcaw2sMNmAqx?cluster=devnet)
- `create_ruleset`: [4HiJDGr1EHG5Vcjkxo22tAhWjsRgwtWvbXTevBbirgYQDRYiS821LpECwUkEHEBV5oQug9PvxGfajNwxHPq4bZiz](https://explorer.solana.com/tx/4HiJDGr1EHG5Vcjkxo22tAhWjsRgwtWvbXTevBbirgYQDRYiS821LpECwUkEHEBV5oQug9PvxGfajNwxHPq4bZiz?cluster=devnet)
- `distribute`: [2gM9fdB99s5p6tUdyUfwcuTCiv4fr9zNVh7mbdTNbW4EAmS5HCk3yr3224pwLSk6bD9B6bGcYgPtZj7amo38vHwJ](https://explorer.solana.com/tx/2gM9fdB99s5p6tUdyUfwcuTCiv4fr9zNVh7mbdTNbW4EAmS5HCk3yr3224pwLSk6bD9B6bGcYgPtZj7amo38vHwJ?cluster=devnet)
- `fail_for_violation`: [4cKcop1TEBYyJp8h9o8p1k7kTZKdX2wH1NGVw66hu2ruVazvvtvPtBk9n9BCfUHv8ThXoDmazjgXMDzY1u1SRwK7](https://explorer.solana.com/tx/4cKcop1TEBYyJp8h9o8p1k7kTZKdX2wH1NGVw66hu2ruVazvvtvPtBk9n9BCfUHv8ThXoDmazjgXMDzY1u1SRwK7?cluster=devnet)
- `hold_account`: [33FTxr2LhEjxjRCKGuDkhSKLq7pFhAZ4cjqPivaKjBaXXdS6WqRrboEnfRWJyWC6HGWgTfNdFiEknk3NdEFGpicB](https://explorer.solana.com/tx/33FTxr2LhEjxjRCKGuDkhSKLq7pFhAZ4cjqPivaKjBaXXdS6WqRrboEnfRWJyWC6HGWgTfNdFiEknk3NdEFGpicB?cluster=devnet)
- `init_config`: [4V33t4BcFXbyPcfBKVsGiPSjdRCVN4ZUeLh9HM74vANnqYmvjGNc7rTCoiNDrRVJrt54wJmDYnmxSizatULFXv4o](https://explorer.solana.com/tx/4V33t4BcFXbyPcfBKVsGiPSjdRCVN4ZUeLh9HM74vANnqYmvjGNc7rTCoiNDrRVJrt54wJmDYnmxSizatULFXv4o?cluster=devnet)
- `init_vault`: [5jx6RA9vai51DuACgQwHjdLe1U1JsYNQb7rDEw99fbtZCk8bBfKSn85HRb2QmY9MgtcrREeGiXma3hpkUvD8w9k4](https://explorer.solana.com/tx/5jx6RA9vai51DuACgQwHjdLe1U1JsYNQb7rDEw99fbtZCk8bBfKSn85HRb2QmY9MgtcrREeGiXma3hpkUvD8w9k4?cluster=devnet)
- `mint_to`: [5sDsaCyJcVqcrFJkDx3U7BeF9BLK8u3xLYUvQP8J9Lk5fW25Z1WGR5Re33NEfYtKCjzNb22jjvmebuyQT3kzxU44](https://explorer.solana.com/tx/5sDsaCyJcVqcrFJkDx3U7BeF9BLK8u3xLYUvQP8J9Lk5fW25Z1WGR5Re33NEfYtKCjzNb22jjvmebuyQT3kzxU44?cluster=devnet)
- `open_account`: [5FntrpmrjQD3KKrcm8Y9d46ciYzHgjDSph8Gw4bJeGWAS3dgX971B9W2UqwFEm7xfQ7yCkwKxzDzYNM6UJD4zeiv](https://explorer.solana.com/tx/5FntrpmrjQD3KKrcm8Y9d46ciYzHgjDSph8Gw4bJeGWAS3dgX971B9W2UqwFEm7xfQ7yCkwKxzDzYNM6UJD4zeiv?cluster=devnet)
- `post_journal`: [4uTUUubVf3Snh6FZqwDM7KRAXTxkAdk45oGRp7D2CFVkvySDz21ikqGcPWRY9fDVJbn21xpHUeKPVPFKz961e5qh](https://explorer.solana.com/tx/4uTUUubVf3Snh6FZqwDM7KRAXTxkAdk45oGRp7D2CFVkvySDz21ikqGcPWRY9fDVJbn21xpHUeKPVPFKz961e5qh?cluster=devnet)
- `register_product`: [3EPzcB7CBTEMeTFkTWGfrN1vkDfX1LntnPn8y5nNEeLdtNrDniKkDYaULNrGpRbQtmD2buMuNBnphtd8kEZ7wCyz](https://explorer.solana.com/tx/3EPzcB7CBTEMeTFkTWGfrN1vkDfX1LntnPn8y5nNEeLdtNrDniKkDYaULNrGpRbQtmD2buMuNBnphtd8kEZ7wCyz?cluster=devnet)
- `report_fail`: [5Peyqaqws8tP85guVrsBQyini44Mu3tn6pS2YHwH6DasWgYQQyaRfFges8iy5uXqobYmMtis8SNKYkfAB8Njfken](https://explorer.solana.com/tx/5Peyqaqws8tP85guVrsBQyini44Mu3tn6pS2YHwH6DasWgYQQyaRfFges8iy5uXqobYmMtis8SNKYkfAB8Njfken?cluster=devnet)
- `request_advance`: [2s18LLMijwb7eaQHnT4j6iXf8gzDczowLgBFiPvhJ6tsNCMzQ3i7FC7dx2cXTYUALdvDdiE47NUEr9ErgZLtpdY7](https://explorer.solana.com/tx/2s18LLMijwb7eaQHnT4j6iXf8gzDczowLgBFiPvhJ6tsNCMzQ3i7FC7dx2cXTYUALdvDdiE47NUEr9ErgZLtpdY7?cluster=devnet)
- `reset_account`: [5U6ggm114cdRDNZ9CCEHgEpw7RQAhoakW3J7BJUL6FwioZfSpbYz8tcuZ9WC3WNYeXNe6JqxGXHAeZZtVgPk3rfC](https://explorer.solana.com/tx/5U6ggm114cdRDNZ9CCEHgEpw7RQAhoakW3J7BJUL6FwioZfSpbYz8tcuZ9WC3WNYeXNe6JqxGXHAeZZtVgPk3rfC?cluster=devnet)
- `touch_activity`: [36XAYJGeFpmRpjeU4Lbhw49wbiUBJ52b7ZQHCBJizB4hgVeoo8VWFR6CJkxiuDFKN5TUzGuuDSr5E28MQT8g24xv](https://explorer.solana.com/tx/36XAYJGeFpmRpjeU4Lbhw49wbiUBJ52b7ZQHCBJizB4hgVeoo8VWFR6CJkxiuDFKN5TUzGuuDSr5E28MQT8g24xv?cluster=devnet)
- `unban_wallet`: [N1KSoTZqJXBfSpTd3L5EVhfg1EWvy4THnvqLQbX9ecEw6Mm3pmCDMeEtFoFtmAZZUBwWQDSMr1i1SB6XRXohYuB](https://explorer.solana.com/tx/N1KSoTZqJXBfSpTd3L5EVhfg1EWvy4THnvqLQbX9ecEw6Mm3pmCDMeEtFoFtmAZZUBwWQDSMr1i1SB6XRXohYuB?cluster=devnet)
- `veto_pass`: [Rn8dUgJUzJQuwvkKcYVVcGe6T9QBXZ9psz9LhPMZmhzWuNwamRSB1ZqFQUe6Asq9iqGRRgnFzbP8jBdutR97PN1](https://explorer.solana.com/tx/Rn8dUgJUzJQuwvkKcYVVcGe6T9QBXZ9psz9LhPMZmhzWuNwamRSB1ZqFQUe6Asq9iqGRRgnFzbP8jBdutR97PN1?cluster=devnet)
