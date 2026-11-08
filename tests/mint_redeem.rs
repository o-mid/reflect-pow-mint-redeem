use reflect_pow_mint_redeem::{derive_vault, Vault, NAV_SCALE};
use solana_program_test::{processor, BanksClient, ProgramTest};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_token::state::{Account as TokenAccount, Mint};

async fn send(banks: &mut BanksClient, payer: &Keypair, ixs: &[Instruction], extra: &[&Keypair]) {
    let hash = banks.get_latest_blockhash().await.unwrap();
    let mut tx = Transaction::new_with_payer(ixs, Some(&payer.pubkey()));
    let mut signers = vec![payer];
    signers.extend_from_slice(extra);
    tx.sign(&signers, hash);
    banks.process_transaction(tx).await.unwrap();
}

async fn create(
    banks: &mut BanksClient,
    payer: &Keypair,
    account: &Keypair,
    space: usize,
    owner: &Pubkey,
    init: Instruction,
) {
    let lamports = banks.get_rent().await.unwrap().minimum_balance(space);
    send(
        banks,
        payer,
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &account.pubkey(),
                lamports,
                space as u64,
                owner,
            ),
            init,
        ],
        &[account],
    )
    .await;
}

async fn raw(banks: &mut BanksClient, key: &Pubkey) -> Vec<u8> {
    banks.get_account(*key).await.unwrap().unwrap().data
}

struct World {
    banks: BanksClient,
    payer: Keypair,
    program_id: Pubkey,
    usdc_mint: Pubkey,
    receipt_mint: Pubkey,
    vault: Pubkey,
    vault_usdc: Pubkey,
    user_usdc: Pubkey,
    user_receipt: Pubkey,
}

async fn world() -> World {
    let program_id = Pubkey::new_unique();
    let mut test = ProgramTest::new(
        "reflect_pow_mint_redeem",
        program_id,
        processor!(reflect_pow_mint_redeem::process_instruction),
    );
    test.add_program(
        "spl_token",
        spl_token::id(),
        processor!(spl_token::processor::Processor::process),
    );
    let (mut banks, payer, _) = test.start().await;
    let usdc_mint = Keypair::new();
    let receipt_mint = Keypair::new();
    let vault_usdc = Keypair::new();
    let user_usdc = Keypair::new();
    let user_receipt = Keypair::new();
    let (vault, _) = derive_vault(&program_id, &payer.pubkey(), &usdc_mint.pubkey());
    create(
        &mut banks,
        &payer,
        &usdc_mint,
        Mint::LEN,
        &spl_token::id(),
        spl_token::instruction::initialize_mint(
            &spl_token::id(),
            &usdc_mint.pubkey(),
            &payer.pubkey(),
            None,
            6,
        )
        .unwrap(),
    )
    .await;
    create(
        &mut banks,
        &payer,
        &receipt_mint,
        Mint::LEN,
        &spl_token::id(),
        spl_token::instruction::initialize_mint(
            &spl_token::id(),
            &receipt_mint.pubkey(),
            &vault,
            None,
            6,
        )
        .unwrap(),
    )
    .await;
    for (account, mint, owner) in [
        (&vault_usdc, usdc_mint.pubkey(), vault),
        (&user_usdc, usdc_mint.pubkey(), payer.pubkey()),
        (&user_receipt, receipt_mint.pubkey(), payer.pubkey()),
    ] {
        create(
            &mut banks,
            &payer,
            account,
            TokenAccount::LEN,
            &spl_token::id(),
            spl_token::instruction::initialize_account3(
                &spl_token::id(),
                &account.pubkey(),
                &mint,
                &owner,
            )
            .unwrap(),
        )
        .await;
    }
    send(
        &mut banks,
        &payer,
        &[initialize_ix(
            &program_id,
            &payer.pubkey(),
            &vault,
            &usdc_mint.pubkey(),
            &receipt_mint.pubkey(),
            &vault_usdc.pubkey(),
        )],
        &[],
    )
    .await;
    World {
        banks,
        payer,
        program_id,
        usdc_mint: usdc_mint.pubkey(),
        receipt_mint: receipt_mint.pubkey(),
        vault,
        vault_usdc: vault_usdc.pubkey(),
        user_usdc: user_usdc.pubkey(),
        user_receipt: user_receipt.pubkey(),
    }
}

impl World {
    fn fund(&self, amount: u64) -> Instruction {
        spl_token::instruction::mint_to(
            &spl_token::id(),
            &self.usdc_mint,
            &self.user_usdc,
            &self.payer.pubkey(),
            &[],
            amount,
        )
        .unwrap()
    }
    fn donate(&self, amount: u64) -> Instruction {
        spl_token::instruction::mint_to(
            &spl_token::id(),
            &self.usdc_mint,
            &self.vault_usdc,
            &self.payer.pubkey(),
            &[],
            amount,
        )
        .unwrap()
    }
    fn deposit(&self, amount: u64) -> Instruction {
        mint_ix(
            &self.program_id,
            &self.payer.pubkey(),
            &self.vault,
            &self.user_usdc,
            &self.vault_usdc,
            &self.user_receipt,
            &self.receipt_mint,
            amount,
        )
    }
    fn redeem(&self, shares: u64) -> Instruction {
        redeem_ix(
            &self.program_id,
            &self.payer.pubkey(),
            &self.vault,
            &self.user_usdc,
            &self.vault_usdc,
            &self.user_receipt,
            &self.receipt_mint,
            shares,
        )
    }
    async fn vault(&mut self) -> Vault {
        Vault::unpack(&raw(&mut self.banks, &self.vault).await).unwrap()
    }
    async fn amount(&mut self, key: &Pubkey) -> u64 {
        TokenAccount::unpack(&raw(&mut self.banks, key).await)
            .unwrap()
            .amount
    }
    async fn supply(&mut self) -> u64 {
        Mint::unpack(&raw(&mut self.banks, &self.receipt_mint).await)
            .unwrap()
            .supply
    }
}

#[tokio::test]
async fn deposit_mints_receipts_and_yield_raises_nav() {
    let mut w = world().await;
    let (fund, deposit) = (w.fund(2_000_000), w.deposit(2_000_000));
    send(&mut w.banks, &w.payer, &[fund, deposit], &[]).await;
    let vault = w.vault().await;
    let supply = w.supply().await;
    assert_eq!(
        (vault.total_shares, vault.nav, supply),
        (2_000_000, NAV_SCALE, 2_000_000)
    );
    let nav = vault.nav;
    let (donate, sync) = (
        w.donate(500_000),
        sync_nav_ix(&w.program_id, &w.vault, &w.vault_usdc),
    );
    send(&mut w.banks, &w.payer, &[donate, sync], &[]).await;
    let vault = w.vault().await;
    assert!(vault.nav > nav);
    assert_eq!((vault.total_assets, vault.nav), (2_500_000, 1_250_000));
    let (fund, deposit) = (w.fund(1_250_000), w.deposit(1_250_000));
    send(&mut w.banks, &w.payer, &[fund, deposit], &[]).await;
    let shares = w.vault().await.total_shares;
    let user_receipt = w.user_receipt;
    assert_eq!(shares, 3_000_000);
    assert_eq!(w.amount(&user_receipt).await, 3_000_000);
}

#[tokio::test]
async fn redeem_burns_receipts_and_pays_pro_rata() {
    let mut w = world().await;
    let (fund, deposit) = (w.fund(2_000_000), w.deposit(2_000_000));
    send(&mut w.banks, &w.payer, &[fund, deposit], &[]).await;
    let (donate, sync) = (
        w.donate(500_000),
        sync_nav_ix(&w.program_id, &w.vault, &w.vault_usdc),
    );
    send(&mut w.banks, &w.payer, &[donate, sync], &[]).await;
    let vault = w.vault().await;
    let shares = 1_000_000u64;
    let expected = shares * vault.total_assets / vault.total_shares;
    let user_usdc = w.user_usdc;
    let usdc_before = w.amount(&user_usdc).await;
    let supply_before = w.supply().await;
    let redeem = w.redeem(shares);
    send(&mut w.banks, &w.payer, &[redeem], &[]).await;
    assert_eq!(w.amount(&user_usdc).await - usdc_before, expected);
    assert_eq!(supply_before - w.supply().await, shares);
    let vault = w.vault().await;
    let claim = vault.total_shares as u128 * vault.nav as u128 / NAV_SCALE as u128;
    assert!(claim <= vault.total_assets as u128);
    let vault_usdc = w.vault_usdc;
    assert_eq!(w.amount(&vault_usdc).await, vault.total_assets);

    let assets_before = vault.total_assets;
    let over = w.redeem(vault.total_shares + 1);
    let hash = w.banks.get_latest_blockhash().await.unwrap();
    let mut tx = Transaction::new_with_payer(&[over], Some(&w.payer.pubkey()));
    tx.sign(&[&w.payer], hash);
    assert!(w.banks.process_transaction(tx).await.is_err());
    let vault = w.vault().await;
    assert_eq!(vault.total_assets, assets_before);
    assert_eq!(w.supply().await, vault.total_shares);
}

fn ix(program_id: &Pubkey, accounts: Vec<AccountMeta>, data: Vec<u8>) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

fn amount_data(tag: u8, amount: u64) -> Vec<u8> {
    let mut data = vec![tag];
    data.extend_from_slice(&amount.to_le_bytes());
    data
}

fn flow_accounts(
    user: &Pubkey,
    vault: &Pubkey,
    user_usdc: &Pubkey,
    vault_usdc: &Pubkey,
    user_receipt: &Pubkey,
    receipt_mint: &Pubkey,
) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(*user, true),
        AccountMeta::new(*vault, false),
        AccountMeta::new(*user_usdc, false),
        AccountMeta::new(*vault_usdc, false),
        AccountMeta::new(*user_receipt, false),
        AccountMeta::new(*receipt_mint, false),
        AccountMeta::new_readonly(spl_token::id(), false),
    ]
}

fn initialize_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    vault: &Pubkey,
    usdc_mint: &Pubkey,
    receipt_mint: &Pubkey,
    vault_usdc: &Pubkey,
) -> Instruction {
    ix(
        program_id,
        vec![
            AccountMeta::new(*authority, true),
            AccountMeta::new(*vault, false),
            AccountMeta::new_readonly(*usdc_mint, false),
            AccountMeta::new_readonly(*receipt_mint, false),
            AccountMeta::new_readonly(*vault_usdc, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        vec![0],
    )
}

fn mint_ix(
    program_id: &Pubkey,
    user: &Pubkey,
    vault: &Pubkey,
    user_usdc: &Pubkey,
    vault_usdc: &Pubkey,
    user_receipt: &Pubkey,
    receipt_mint: &Pubkey,
    amount: u64,
) -> Instruction {
    ix(
        program_id,
        flow_accounts(
            user,
            vault,
            user_usdc,
            vault_usdc,
            user_receipt,
            receipt_mint,
        ),
        amount_data(1, amount),
    )
}

fn redeem_ix(
    program_id: &Pubkey,
    user: &Pubkey,
    vault: &Pubkey,
    user_usdc: &Pubkey,
    vault_usdc: &Pubkey,
    user_receipt: &Pubkey,
    receipt_mint: &Pubkey,
    shares: u64,
) -> Instruction {
    ix(
        program_id,
        flow_accounts(
            user,
            vault,
            user_usdc,
            vault_usdc,
            user_receipt,
            receipt_mint,
        ),
        amount_data(2, shares),
    )
}

fn sync_nav_ix(program_id: &Pubkey, vault: &Pubkey, vault_usdc: &Pubkey) -> Instruction {
    ix(
        program_id,
        vec![
            AccountMeta::new(*vault, false),
            AccountMeta::new_readonly(*vault_usdc, false),
        ],
        vec![3],
    )
}
