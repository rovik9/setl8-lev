# Devnet demo: one challenge from purchase to payout

Public data only. Every link opens the transaction or account on Solana Explorer (devnet).

## 1. The challenge purchase (open_account)

The trader paid the challenge price plus the 0.4 % fee in the test stablecoin in one transaction; the vault split the price between its payout pool and the platform wallet, and recorded the challenge.

- open_account A1: [5FntrpmrjQD3KKrcm8Y9d46ciYzHgjDSph8Gw4bJeGWAS3dgX971B9W2UqwFEm7xfQ7yCkwKxzDzYNM6UJD4zeiv](https://explorer.solana.com/tx/5FntrpmrjQD3KKrcm8Y9d46ciYzHgjDSph8Gw4bJeGWAS3dgX971B9W2UqwFEm7xfQ7yCkwKxzDzYNM6UJD4zeiv?cluster=devnet)
- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)
- vault state: [G7qpP8L2kXGP7VYuvhioEQLEuWxUngxDmxqxEqnpUCbT](https://explorer.solana.com/address/G7qpP8L2kXGP7VYuvhioEQLEuWxUngxDmxqxEqnpUCbT?cluster=devnet)
- payout pool: [UjqJZRCALei1whzfLJbX3J5p22hRyYrVF7v3xNPiKPi](https://explorer.solana.com/address/UjqJZRCALei1whzfLJbX3J5p22hRyYrVF7v3xNPiKPi?cluster=devnet)
- test stablecoin mint: [2kBKxj3TYz2UbQfVcJ5xBbWscC5hrALq7MLzJi3c1fTx](https://explorer.solana.com/address/2kBKxj3TYz2UbQfVcJ5xBbWscC5hrALq7MLzJi3c1fTx?cluster=devnet)

## 2. The first journal batches (the winning history)

The evaluator posted the trader's signed trades as journal batches; the program re-checked every rule on chain and the account's equity and sequence advanced only through them.

- post_journal ev=5 pr=3 A1: [4uTUUubVf3Snh6FZqwDM7KRAXTxkAdk45oGRp7D2CFVkvySDz21ikqGcPWRY9fDVJbn21xpHUeKPVPFKz961e5qh](https://explorer.solana.com/tx/4uTUUubVf3Snh6FZqwDM7KRAXTxkAdk45oGRp7D2CFVkvySDz21ikqGcPWRY9fDVJbn21xpHUeKPVPFKz961e5qh?cluster=devnet)
- post_journal ev=4 pr=3 A1: [4smRN8MKiFdjBEmGV852LUpPrtTvYH4LN7oWpWNzRVhmwBp9SMuGGWMwuUYhfis4AaBiU9cuMCQvoNakt8pWLuUn](https://explorer.solana.com/tx/4smRN8MKiFdjBEmGV852LUpPrtTvYH4LN7oWpWNzRVhmwBp9SMuGGWMwuUYhfis4AaBiU9cuMCQvoNakt8pWLuUn?cluster=devnet)
- post_journal ev=4 pr=3 A1: [5CHhUkRP2Er1wuPhrF2Ge6M13ozDbKBcZdQ4LmW5QbYp8L5AqUk7fnUpKPrTagtCe7L8rPqnrHHVTJZsCukaE7b3](https://explorer.solana.com/tx/5CHhUkRP2Er1wuPhrF2Ge6M13ozDbKBcZdQ4LmW5QbYp8L5AqUk7fnUpKPrTagtCe7L8rPqnrHHVTJZsCukaE7b3?cluster=devnet)
- post_journal ev=4 pr=3 A1: [36h5d1mCTnYnZDYcSGP7EWpbqg72jYFbui53eJiY4ynHfrvosjw1PqK2W8XRYBKJoTGfM9yN22w7LEmBtELJDbfM](https://explorer.solana.com/tx/36h5d1mCTnYnZDYcSGP7EWpbqg72jYFbui53eJiY4ynHfrvosjw1PqK2W8XRYBKJoTGfM9yN22w7LEmBtELJDbfM?cluster=devnet)
- post_journal ev=5 pr=1 A1: [3KpJdiSxC8RUiD795P3zQgMW16Ctb1KhP5VhB7SfwABRoixT8Yfngn2hL5kYX7wFVbW15R18v98SvGK5uueY42iS](https://explorer.solana.com/tx/3KpJdiSxC8RUiD795P3zQgMW16Ctb1KhP5VhB7SfwABRoixT8Yfngn2hL5kYX7wFVbW15R18v98SvGK5uueY42iS?cluster=devnet)
- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)

## 3. Phase 1 passed: request_advance and confirm_pass

The account met the phase-1 target; the trader asked to advance and the evaluator confirmed, so the program moved the account to phase 2.

- request_advance A1: [2s18LLMijwb7eaQHnT4j6iXf8gzDczowLgBFiPvhJ6tsNCMzQ3i7FC7dx2cXTYUALdvDdiE47NUEr9ErgZLtpdY7](https://explorer.solana.com/tx/2s18LLMijwb7eaQHnT4j6iXf8gzDczowLgBFiPvhJ6tsNCMzQ3i7FC7dx2cXTYUALdvDdiE47NUEr9ErgZLtpdY7?cluster=devnet)
- confirm_pass A1: [U7H8c65YTkgjhMYfkd1AB7bCfCmD7m2jC7Se2pXEPQ18vCghYDZUczLKZvRJQfPuKvKTJZ45YrZ2wSoYGzR8Rvk](https://explorer.solana.com/tx/U7H8c65YTkgjhMYfkd1AB7bCfCmD7m2jC7Se2pXEPQ18vCghYDZUczLKZvRJQfPuKvKTJZ45YrZ2wSoYGzR8Rvk?cluster=devnet)
- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)

## 4. Phase 2 passed: request_advance and confirm_pass

The account met the phase-2 target and the pass is confirmed; the program now holds it for the one-day pass delay before the payout can be queued.

- request_advance A1: [4sJZBJqxJ4uDz8ahap8bRj7wyZNU2ZgGB7r8tmWxqGyTqWCZctNxdgwiaQ9gNy1TcujbTD54rhDBCFMy7PuwPMnt](https://explorer.solana.com/tx/4sJZBJqxJ4uDz8ahap8bRj7wyZNU2ZgGB7r8tmWxqGyTqWCZctNxdgwiaQ9gNy1TcujbTD54rhDBCFMy7PuwPMnt?cluster=devnet)
- confirm_pass A1: [sZYPy7wnV4NsXJmFgwUEJcMY9pMagV5v7oVSyinQN3m9Lk1ryHd5QKej5DXHcxaLWdJvUykPeFD4QjrHm9efc4i](https://explorer.solana.com/tx/sZYPy7wnV4NsXJmFgwUEJcMY9pMagV5v7oVSyinQN3m9Lk1ryHd5QKej5DXHcxaLWdJvUykPeFD4QjrHm9efc4i?cluster=devnet)
- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)

## 5. finalize_pass and the queued payout claim

After the pass delay the program started the funded phase and the vault queued a payout claim for the trader; the claim account is owned by the vault.

*not yet: it exists after day 2, from 2026-11-01 12:20:34 UTC (unix second 1793535634); run `run` again then*

- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)

## 6. The heartbeat and the settlement

Anyone can run the vault's heartbeat: it reconciled the vault's books with the trading program's tally, froze the pool balances, paid the claim to the trader's token account and closed the cycle.

*not yet: it exists after day 2, from 2026-11-01 12:20:34 UTC (unix second 1793535634) and the payout step; run `run` again then*

- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)
- payout pool: [UjqJZRCALei1whzfLJbX3J5p22hRyYrVF7v3xNPiKPi](https://explorer.solana.com/address/UjqJZRCALei1whzfLJbX3J5p22hRyYrVF7v3xNPiKPi?cluster=devnet)

## 7. The trader's balance before and after

The payout landed in the trader's own token account: the balance rose by the paid amount (the claim, paid in full when the pool covers the queue).

*not yet: after the payout step*

- A1 trading account: [A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU](https://explorer.solana.com/address/A3ncHBKQRrRNKVaeH9WuhMmwZ22BNoKyeCFsxK6ZwpvU?cluster=devnet)
- A1 trader wallet: [7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF](https://explorer.solana.com/address/7urBRSkXz6jAoz7j4QnVjgWWVLUxV6nzjZxNEWhxkLeF?cluster=devnet)
