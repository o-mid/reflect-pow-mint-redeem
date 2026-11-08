use reflect_pow_mint_redeem::{derive_vault, quote_assets, quote_shares, Vault};
use solana_program_test::{processor, BanksClient, BanksClientError, ProgramTest};
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

async fn try_send(
    banks: &mut BanksClient,
    payer: &Keypair,
    ixs: &[Instruction],
) -> Result<(), BanksClientError> {
    let hash = banks.get_latest_blockhash().await.unwrap();
    let mut tx = Transaction::new_with_payer(ixs, Some(&payer.pubkey()));
    tx.sign(&[payer], hash);
    banks.process_transaction(tx).await
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
    world_with(0).await
}

async fn world_with(prefund: u64) -> World {
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
    if prefund > 0 {
        send(
            &mut banks,
            &payer,
            &[spl_token::instruction::mint_to(
                &spl_token::id(),
                &usdc_mint.pubkey(),
                &vault_usdc.pubkey(),
                &payer.pubkey(),
                &[],
                prefund,
            )
            .unwrap()],
            &[],
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
async fn deposit_mints_receipts_and_yield_raises_claim() {
    let mut w = world().await;
    let deposit = 2_000_000u64;
    let (fund, mint) = (w.fund(deposit), w.deposit(deposit));
    send(&mut w.banks, &w.payer, &[fund, mint], &[]).await;
    let vault = w.vault().await;
    let shares = quote_shares(deposit, 0, 0).unwrap();
    assert_eq!(vault.total_shares, shares);
    assert_eq!(w.supply().await, shares);
    assert_eq!(
        quote_assets(shares, vault.total_assets, vault.total_shares).unwrap(),
        deposit
    );
    let (donate, sync) = (
        w.donate(500_000),
        sync_nav_ix(&w.program_id, &w.vault, &w.vault_usdc),
    );
    send(&mut w.banks, &w.payer, &[donate, sync], &[]).await;
    let vault = w.vault().await;
    let claim = quote_assets(shares, vault.total_assets, vault.total_shares).unwrap();
    assert!(claim > deposit);
    assert_eq!(vault.total_assets, deposit + 500_000);
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
    let shares = vault.total_shares / 2;
    let expected = quote_assets(shares, vault.total_assets, vault.total_shares).unwrap();
    let user_usdc = w.user_usdc;
    let usdc_before = w.amount(&user_usdc).await;
    let supply_before = w.supply().await;
    let redeem = w.redeem(shares);
    send(&mut w.banks, &w.payer, &[redeem], &[]).await;
    assert_eq!(w.amount(&user_usdc).await - usdc_before, expected);
    assert_eq!(supply_before - w.supply().await, shares);
    let vault = w.vault().await;
    let rest = quote_assets(vault.total_shares, vault.total_assets, vault.total_shares).unwrap();
    assert!(rest <= vault.total_assets);
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

#[tokio::test]
async fn donation_before_first_mint_is_not_captured() {
    let donation = 1_000_000u64;
    let mut w = world_with(donation).await;
    let booked = w.vault().await;
    assert_eq!((booked.total_assets, booked.total_shares), (donation, 0));

    let (fund_dust, dust) = (w.fund(1), w.deposit(1));
    assert!(try_send(&mut w.banks, &w.payer, &[fund_dust, dust])
        .await
        .is_err());
    assert_eq!(w.vault().await.total_assets, donation);
    let user_receipt = w.user_receipt;
    assert_eq!(w.amount(&user_receipt).await, 0);

    let deposit = 2_000_000u64;
    let shares = quote_shares(deposit, donation, 0).unwrap();
    let (fund, mint) = (w.fund(deposit), w.deposit(deposit));
    send(&mut w.banks, &w.payer, &[fund, mint], &[]).await;
    assert_eq!(w.amount(&user_receipt).await, shares);
    let vault = w.vault().await;
    let payout = quote_assets(shares, vault.total_assets, vault.total_shares).unwrap();
    assert!(payout <= deposit);
    let user_usdc = w.user_usdc;
    let before = w.amount(&user_usdc).await;
    let redeem = w.redeem(shares);
    send(&mut w.banks, &w.payer, &[redeem], &[]).await;
    assert_eq!(w.amount(&user_usdc).await - before, payout);
    let left = w.vault().await;
    assert!(left.total_assets >= donation);
    assert_eq!(left.total_shares, 0);
    let vault_usdc = w.vault_usdc;
    assert_eq!(w.amount(&vault_usdc).await, left.total_assets);
}

#[tokio::test]
async fn sync_after_donation_does_not_mint_one_for_one() {
    let mut w = world().await;
    let donation = 1_000_000u64;
    let (donate, sync) = (
        w.donate(donation),
        sync_nav_ix(&w.program_id, &w.vault, &w.vault_usdc),
    );
    send(&mut w.banks, &w.payer, &[donate, sync], &[]).await;
    assert_eq!(w.vault().await.total_shares, 0);
    let (fund_dust, dust) = (w.fund(1), w.deposit(1));
    assert!(try_send(&mut w.banks, &w.payer, &[fund_dust, dust])
        .await
        .is_err());
    let vault_usdc = w.vault_usdc;
    assert_eq!(w.amount(&vault_usdc).await, donation);

    let deposit = donation + 1;
    let shares = quote_shares(deposit, donation, 0).unwrap();
    let (fund, mint) = (w.fund(deposit), w.deposit(deposit));
    send(&mut w.banks, &w.payer, &[fund, mint], &[]).await;
    let vault = w.vault().await;
    let payout = quote_assets(shares, vault.total_assets, vault.total_shares).unwrap();
    assert!(payout <= deposit);
    assert!(vault.total_assets - payout >= donation);
}

#[tokio::test]
async fn initialize_rejects_seeded_or_freezable_receipt_mint() {
    assert!(bad_receipt_mint(true, false).await.is_err());
    assert!(bad_receipt_mint(false, true).await.is_err());
}

async fn bad_receipt_mint(seed_supply: bool, freeze: bool) -> Result<(), BanksClientError> {
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
    let user_receipt = Keypair::new();
    let (vault, _) = derive_vault(&program_id, &payer.pubkey(), &usdc_mint.pubkey());
    let freeze_auth = if freeze { Some(payer.pubkey()) } else { None };
    let mint_authority = if seed_supply { payer.pubkey() } else { vault };
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
            &mint_authority,
            freeze_auth.as_ref(),
            6,
        )
        .unwrap(),
    )
    .await;
    create(
        &mut banks,
        &payer,
        &vault_usdc,
        TokenAccount::LEN,
        &spl_token::id(),
        spl_token::instruction::initialize_account3(
            &spl_token::id(),
            &vault_usdc.pubkey(),
            &usdc_mint.pubkey(),
            &vault,
        )
        .unwrap(),
    )
    .await;
    if seed_supply {
        create(
            &mut banks,
            &payer,
            &user_receipt,
            TokenAccount::LEN,
            &spl_token::id(),
            spl_token::instruction::initialize_account3(
                &spl_token::id(),
                &user_receipt.pubkey(),
                &receipt_mint.pubkey(),
                &payer.pubkey(),
            )
            .unwrap(),
        )
        .await;
        send(
            &mut banks,
            &payer,
            &[
                spl_token::instruction::mint_to(
                    &spl_token::id(),
                    &receipt_mint.pubkey(),
                    &user_receipt.pubkey(),
                    &payer.pubkey(),
                    &[],
                    1,
                )
                .unwrap(),
                spl_token::instruction::set_authority(
                    &spl_token::id(),
                    &receipt_mint.pubkey(),
                    Some(&vault),
                    spl_token::instruction::AuthorityType::MintTokens,
                    &payer.pubkey(),
                    &[],
                )
                .unwrap(),
            ],
            &[],
        )
        .await;
    }
    try_send(
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
    )
    .await
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
