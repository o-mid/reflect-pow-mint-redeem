#![allow(unexpected_cfgs)]

use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_option::COption,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction,
    sysvar::Sysvar,
};
use spl_token::{
    instruction as token_ix,
    state::{Account as TokenAccount, Mint},
};

pub const NAV_SCALE: u64 = 1_000_000;
/// Virtual shares offset the empty-vault price so a donated balance is not
/// captured by the first depositor. Matches a 6-decimal offset.
pub const VIRTUAL_SHARES: u64 = 1_000_000;
pub const VIRTUAL_ASSETS: u64 = 1;

pub enum VaultError {
    NotInitialized = 1,
    ZeroAmount = 2,
    BadAccount = 3,
    Insolvent = 4,
}

impl From<VaultError> for ProgramError {
    fn from(e: VaultError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq)]
pub struct Vault {
    pub is_initialized: u8,
    pub bump: u8,
    pub authority: Pubkey,
    pub usdc_mint: Pubkey,
    pub receipt_mint: Pubkey,
    pub vault_usdc: Pubkey,
    pub total_assets: u64,
    pub total_shares: u64,
    pub nav: u64,
}

impl Vault {
    pub const LEN: usize = 1 + 1 + 32 * 4 + 8 * 3;

    pub fn unpack(data: &[u8]) -> Result<Self, ProgramError> {
        Self::try_from_slice(data).map_err(|_| ProgramError::InvalidAccountData)
    }

    fn refresh_nav(&mut self) -> ProgramResult {
        self.nav = if self.total_shares == 0 {
            NAV_SCALE
        } else {
            let nav = (self.total_assets as u128)
                .checked_mul(NAV_SCALE as u128)
                .ok_or(ProgramError::ArithmeticOverflow)?
                / self.total_shares as u128;
            u64::try_from(nav).map_err(|_| ProgramError::ArithmeticOverflow)?
        };
        Ok(())
    }
}

pub fn derive_vault(program_id: &Pubkey, authority: &Pubkey, usdc_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"vault", authority.as_ref(), usdc_mint.as_ref()],
        program_id,
    )
}

fn signer_seeds<'a>(authority: &'a Pubkey, usdc_mint: &'a Pubkey, bump: &'a u8) -> [&'a [u8]; 4] {
    [
        b"vault",
        authority.as_ref(),
        usdc_mint.as_ref(),
        std::slice::from_ref(bump),
    ]
}

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let (tag, rest) = data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match tag {
        0 => initialize(program_id, accounts),
        1 => exchange(program_id, accounts, read_u64(rest)?, true),
        2 => exchange(program_id, accounts, read_u64(rest)?, false),
        3 => sync_nav(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn mul_div(amount: u64, numer: u64, denom: u64) -> Result<u64, ProgramError> {
    if denom == 0 {
        return Err(VaultError::Insolvent.into());
    }
    let value = (amount as u128)
        .checked_mul(numer as u128)
        .ok_or(ProgramError::ArithmeticOverflow)?
        / denom as u128;
    u64::try_from(value).map_err(|_| ProgramError::ArithmeticOverflow)
}

pub fn quote_shares(
    amount: u64,
    total_assets: u64,
    total_shares: u64,
) -> Result<u64, ProgramError> {
    if amount == 0 {
        return Err(VaultError::ZeroAmount.into());
    }
    let shares = mul_div(
        amount,
        total_shares
            .checked_add(VIRTUAL_SHARES)
            .ok_or(ProgramError::ArithmeticOverflow)?,
        total_assets
            .checked_add(VIRTUAL_ASSETS)
            .ok_or(ProgramError::ArithmeticOverflow)?,
    )?;
    if shares == 0 {
        Err(VaultError::ZeroAmount.into())
    } else {
        Ok(shares)
    }
}

pub fn quote_assets(
    shares: u64,
    total_assets: u64,
    total_shares: u64,
) -> Result<u64, ProgramError> {
    if shares == 0 || total_shares == 0 || shares > total_shares {
        return Err(if shares == 0 {
            VaultError::ZeroAmount
        } else {
            VaultError::Insolvent
        }
        .into());
    }
    let assets = mul_div(
        shares,
        total_assets
            .checked_add(VIRTUAL_ASSETS)
            .ok_or(ProgramError::ArithmeticOverflow)?,
        total_shares
            .checked_add(VIRTUAL_SHARES)
            .ok_or(ProgramError::ArithmeticOverflow)?,
    )?;
    if assets == 0 {
        Err(VaultError::ZeroAmount.into())
    } else {
        Ok(assets.min(total_assets))
    }
}

fn read_u64(data: &[u8]) -> Result<u64, ProgramError> {
    data.get(..8)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ProgramError::InvalidInstructionData)
}

fn load(program_id: &Pubkey, account: &AccountInfo) -> Result<Vault, ProgramError> {
    if account.owner != program_id {
        return Err(ProgramError::IncorrectProgramId);
    }
    let vault = Vault::unpack(&account.data.borrow())?;
    if vault.is_initialized != 1 {
        return Err(VaultError::NotInitialized.into());
    }
    let (pda, _) = derive_vault(program_id, &vault.authority, &vault.usdc_mint);
    if account.key != &pda {
        return Err(VaultError::BadAccount.into());
    }
    Ok(vault)
}

fn save(account: &AccountInfo, vault: &Vault) -> ProgramResult {
    vault
        .serialize(&mut &mut account.data.borrow_mut()[..])
        .map_err(|_| ProgramError::InvalidAccountData)
}

fn initialize(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let accounts = &mut accounts.iter();
    let authority = next_account_info(accounts)?;
    let vault_ai = next_account_info(accounts)?;
    let usdc_mint = next_account_info(accounts)?;
    let receipt_mint = next_account_info(accounts)?;
    let vault_usdc = next_account_info(accounts)?;
    let system_program = next_account_info(accounts)?;
    if !authority.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if system_program.key != &solana_program::system_program::id() {
        return Err(ProgramError::IncorrectProgramId);
    }
    let (pda, bump) = derive_vault(program_id, authority.key, usdc_mint.key);
    if vault_ai.key != &pda {
        return Err(VaultError::BadAccount.into());
    }
    if receipt_mint.owner != &spl_token::id()
        || usdc_mint.owner != &spl_token::id()
        || vault_usdc.owner != &spl_token::id()
    {
        return Err(VaultError::BadAccount.into());
    }
    let mint = Mint::unpack(&receipt_mint.data.borrow())?;
    if mint.mint_authority != COption::Some(pda)
        || mint.supply != 0
        || mint.freeze_authority != COption::None
    {
        return Err(VaultError::BadAccount.into());
    }
    let usdc_acc = TokenAccount::unpack(&vault_usdc.data.borrow())?;
    if usdc_acc.owner != pda || usdc_acc.mint != *usdc_mint.key {
        return Err(VaultError::BadAccount.into());
    }
    invoke_signed(
        &system_instruction::create_account(
            authority.key,
            vault_ai.key,
            Rent::get()?.minimum_balance(Vault::LEN),
            Vault::LEN as u64,
            program_id,
        ),
        &[authority.clone(), vault_ai.clone(), system_program.clone()],
        &[&signer_seeds(authority.key, usdc_mint.key, &bump)],
    )?;
    save(
        vault_ai,
        &Vault {
            is_initialized: 1,
            bump,
            authority: *authority.key,
            usdc_mint: *usdc_mint.key,
            receipt_mint: *receipt_mint.key,
            vault_usdc: *vault_usdc.key,
            total_assets: usdc_acc.amount,
            total_shares: 0,
            nav: NAV_SCALE,
        },
    )
}

fn exchange(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    amount: u64,
    is_mint: bool,
) -> ProgramResult {
    let accounts = &mut accounts.iter();
    let user = next_account_info(accounts)?;
    let vault_ai = next_account_info(accounts)?;
    let user_usdc = next_account_info(accounts)?;
    let vault_usdc = next_account_info(accounts)?;
    let user_receipt = next_account_info(accounts)?;
    let receipt_mint = next_account_info(accounts)?;
    let token_program = next_account_info(accounts)?;
    if !user.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if token_program.key != &spl_token::id() {
        return Err(ProgramError::IncorrectProgramId);
    }
    let mut vault = load(program_id, vault_ai)?;
    if vault_usdc.key != &vault.vault_usdc || receipt_mint.key != &vault.receipt_mint {
        return Err(VaultError::BadAccount.into());
    }
    for (account, mint, owner) in [
        (user_usdc, &vault.usdc_mint, user.key),
        (user_receipt, &vault.receipt_mint, user.key),
    ] {
        let token = TokenAccount::unpack(&account.data.borrow())?;
        if token.mint != *mint || token.owner != *owner {
            return Err(VaultError::BadAccount.into());
        }
    }
    let (bump, authority, usdc_mint) = (vault.bump, vault.authority, vault.usdc_mint);
    let seeds = signer_seeds(&authority, &usdc_mint, &bump);
    if is_mint {
        let shares = quote_shares(amount, vault.total_assets, vault.total_shares)?;
        invoke(
            &token_ix::transfer(
                token_program.key,
                user_usdc.key,
                vault_usdc.key,
                user.key,
                &[],
                amount,
            )?,
            &[
                user_usdc.clone(),
                vault_usdc.clone(),
                user.clone(),
                token_program.clone(),
            ],
        )?;
        invoke_signed(
            &token_ix::mint_to(
                token_program.key,
                receipt_mint.key,
                user_receipt.key,
                vault_ai.key,
                &[],
                shares,
            )?,
            &[
                receipt_mint.clone(),
                user_receipt.clone(),
                vault_ai.clone(),
                token_program.clone(),
            ],
            &[&seeds],
        )?;
        vault.total_assets = vault
            .total_assets
            .checked_add(amount)
            .ok_or(ProgramError::ArithmeticOverflow)?;
        vault.total_shares = vault
            .total_shares
            .checked_add(shares)
            .ok_or(ProgramError::ArithmeticOverflow)?;
    } else {
        let assets = quote_assets(amount, vault.total_assets, vault.total_shares)?;
        invoke(
            &token_ix::burn(
                token_program.key,
                user_receipt.key,
                receipt_mint.key,
                user.key,
                &[],
                amount,
            )?,
            &[
                user_receipt.clone(),
                receipt_mint.clone(),
                user.clone(),
                token_program.clone(),
            ],
        )?;
        invoke_signed(
            &token_ix::transfer(
                token_program.key,
                vault_usdc.key,
                user_usdc.key,
                vault_ai.key,
                &[],
                assets,
            )?,
            &[
                vault_usdc.clone(),
                user_usdc.clone(),
                vault_ai.clone(),
                token_program.clone(),
            ],
            &[&seeds],
        )?;
        vault.total_assets = vault
            .total_assets
            .checked_sub(assets)
            .ok_or(ProgramError::ArithmeticOverflow)?;
        vault.total_shares = vault
            .total_shares
            .checked_sub(amount)
            .ok_or(ProgramError::ArithmeticOverflow)?;
    }
    vault.refresh_nav()?;
    save(vault_ai, &vault)
}

fn sync_nav(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let accounts = &mut accounts.iter();
    let vault_ai = next_account_info(accounts)?;
    let vault_usdc = next_account_info(accounts)?;
    let mut vault = load(program_id, vault_ai)?;
    if vault_usdc.key != &vault.vault_usdc {
        return Err(VaultError::BadAccount.into());
    }
    let balance = TokenAccount::unpack(&vault_usdc.data.borrow())?.amount;
    if balance < vault.total_assets {
        return Err(VaultError::Insolvent.into());
    }
    vault.total_assets = balance;
    vault.refresh_nav()?;
    save(vault_ai, &vault)
}
