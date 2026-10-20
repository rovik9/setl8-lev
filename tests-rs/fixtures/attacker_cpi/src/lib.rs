//! TEST-ONLY. Accounts: `[0]` the program to call (the vault), `[1..]` the accounts of the inner instruction in order. The
//! account at `[1]` is the `sector_authority` slot. Data: the raw data of the inner instruction. The program CPIs with the
//! inner accounts, marks `[1]` as a signer, and signs with the seeds `["setl8_sector_authority", bump]` where the bump is the
//! canonical one under THIS program's id (the only id `invoke_signed` can sign for). It cannot sign for any other program's PDA.
use solana_program::{
    account_info::AccountInfo,
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    pubkey::Pubkey,
};

entrypoint!(process);

pub const SEED: &[u8] = b"setl8_sector_authority";

fn process(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (_pda, bump) = Pubkey::find_program_address(&[SEED], program_id);
    let metas: Vec<AccountMeta> = accounts[1..]
        .iter()
        .enumerate()
        .map(|(i, a)| AccountMeta { pubkey: *a.key, is_signer: i == 0 || a.is_signer, is_writable: a.is_writable })
        .collect();
    let ix = Instruction { program_id: *accounts[0].key, accounts: metas, data: data.to_vec() };
    invoke_signed(&ix, accounts, &[&[SEED, &[bump]]])
}
