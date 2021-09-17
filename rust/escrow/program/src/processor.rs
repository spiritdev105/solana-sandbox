use {
  crate::{error, instruction::EscrowInstruction, state::Escrow},
  solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::{IsInitialized, Pack},
    pubkey::Pubkey,
    sysvar::{rent::Rent, Sysvar},
  },
  spl_token::{instruction, state::Account as TokenAccount},
};

fn assert_owned_by(account: &AccountInfo, owner: &Pubkey) -> ProgramResult {
  if account.owner != owner {
    Err(ProgramError::IncorrectProgramId)
  } else {
    Ok(())
  }
}

pub struct Processor;
impl Processor {
  pub fn process(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
  ) -> ProgramResult {
    let instruction = EscrowInstruction::unpack(instruction_data)?;

    match instruction {
      EscrowInstruction::InitEscrow { amount } => {
        msg!("Instruction: Init Escrow");
        Self::process_init_escrow(program_id, accounts, amount)
      }
      EscrowInstruction::Exchange { amount } => {
        msg!("Instruction: Init Escrow");
        Self::process_exchange(program_id, accounts, amount)
      }
    }
  }

  pub fn process_init_escrow(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    amount: u64,
  ) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();
    let initializer = next_account_info(account_info_iter)?;

    if !initializer.is_signer {
      return Err(ProgramError::MissingRequiredSignature);
    }

    let temp_token_account = next_account_info(account_info_iter)?;
    let token_to_receive_account = next_account_info(account_info_iter)?;

    // Make sure Token Account is owned by Token Program
    assert_owned_by(token_to_receive_account, &spl_token::id())?;

    let escrow_account: &AccountInfo = next_account_info(account_info_iter)?;

    let rent: &Rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;

    if !rent.is_exempt(escrow_account.lamports(), escrow_account.data_len()) {
      return Err(error::EscrowError::NotRentExmpt.into());
    }

    let mut escrow_info: Escrow = Escrow::unpack_unchecked(&escrow_account.data.borrow())?;
    if escrow_info.is_initialized() {
      return Err(ProgramError::AccountAlreadyInitialized);
    }

    escrow_info.is_initialized = true;
    escrow_info.initializer_pubkey = *initializer.key;
    escrow_info.temp_token_account_pubkey = *temp_token_account.key;
    escrow_info.initializer_token_to_receive_account_pubkey = *token_to_receive_account.key;
    escrow_info.expected_amount = amount;

    Escrow::pack(escrow_info, &mut escrow_account.data.borrow_mut())?;

    let escrow_seed = &["escrow".as_bytes(), program_id.as_ref()];

    // Program derived address for Cross Program Invocation
    let (pda_key, _bump_seed) = Pubkey::find_program_address(escrow_seed, program_id);

    let token_program: &AccountInfo = next_account_info(account_info_iter)?;

    // token_program_id: &Pubkey,
    // owned_pubkey: &Pubkey,
    // new_authority_pubkey: Option<&Pubkey>,
    // authority_type: AuthorityType,
    // owner_pubkey: &Pubkey,
    // signer_pubkeys: &[&Pubkey])

    let owner_change_instruction = instruction::set_authority(
      token_program.key,
      temp_token_account.key,
      Some(&pda_key),
      instruction::AuthorityType::AccountOwner,
      initializer.key,
      &[&initializer.key],
    )?;

    // Transfer temporary token account ownership to PDA
    invoke(
      &owner_change_instruction,
      &[
        temp_token_account.clone(),
        initializer.clone(),
        token_program.clone(),
      ],
    )?;

    Ok(())
  }

  fn process_exchange(program_id: &Pubkey, accounts: &[AccountInfo], amount: u64) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();
    let taker: &AccountInfo = next_account_info(account_info_iter)?;
    if !taker.is_signer {
      return Err(ProgramError::MissingRequiredSignature);
    }

    let taker_sending_token_account = next_account_info(account_info_iter)?;
    let taker_receive_token_account = next_account_info(account_info_iter)?;

    let pda_temp_token_account: &AccountInfo = next_account_info(account_info_iter)?;
    let pda_temp_token_account_info: TokenAccount =
      TokenAccount::unpack(&pda_temp_token_account.data.borrow())?;

    let escrow_seed = &["escrow".as_bytes(), program_id.as_ref()];

    // Find a valid program address and its corresponding bump seed which must be passed as an additional seed when calling invoke_signed
    let (pda_key, bump_seed) = Pubkey::find_program_address(escrow_seed, program_id);

    if amount != pda_temp_token_account_info.amount {
      return Err(error::EscrowError::ExpectedAmountMismatch.into());
    }

    let initializer_main_account: &AccountInfo = next_account_info(account_info_iter)?;

    let initializer_receive_token_account: &AccountInfo = next_account_info(account_info_iter)?;

    let escrow_account: &AccountInfo = next_account_info(account_info_iter)?;

    let escrow_info: Escrow = Escrow::unpack(&escrow_account.data.borrow())?;

    if escrow_info.temp_token_account_pubkey != *pda_temp_token_account.key {
      return Err(ProgramError::InvalidAccountData);
    }

    if escrow_info.initializer_pubkey != *initializer_main_account.key {
      return Err(ProgramError::InvalidAccountData);
    }

    if escrow_info.initializer_token_to_receive_account_pubkey
      != *initializer_receive_token_account.key
    {
      return Err(ProgramError::InvalidAccountData);
    }

    let token_program = next_account_info(account_info_iter)?;

    let transfer_to_initializer_instruction = spl_token::instruction::transfer(
      token_program.key,
      taker_sending_token_account.key,
      initializer_receive_token_account.key,
      taker.key,
      &[&taker.key],
      escrow_info.expected_amount,
    )?;

    // transfer tokens to initializer's receive token account
    invoke(
      &transfer_to_initializer_instruction,
      &[
        taker_sending_token_account.clone(),
        initializer_receive_token_account.clone(),
        taker.clone(),
        token_program.clone(),
      ],
    )?;

    let pda_account = next_account_info(account_info_iter)?;

    let transfer_to_taker_instruction = spl_token::instruction::transfer(
      token_program.key,
      pda_temp_token_account.key,
      taker_receive_token_account.key,
      &pda_key,
      &[&pda_key],
      pda_temp_token_account_info.amount,
    )?;

    // Signer seeds to let pda invoke program as pda does not own private key
    let signers_seeds = &["escrow".as_bytes(), program_id.as_ref(), &[bump_seed]];
    // transfer tokens to taker's receive token account
    invoke_signed(
      &transfer_to_taker_instruction,
      &[
        pda_temp_token_account.clone(),
        taker_receive_token_account.clone(),
        pda_account.clone(),
        token_program.clone(),
      ],
      &[signers_seeds],
    )?;

    let close_pda_temp_account_instruction = spl_token::instruction::close_account(
      token_program.key,
      pda_temp_token_account.key,
      initializer_main_account.key,
      &pda_key,
      &[&pda_key],
    )?;
    // close pda temp account
    invoke_signed(
      &close_pda_temp_account_instruction,
      &[
        pda_temp_token_account.clone(),
        initializer_main_account.clone(),
        pda_account.clone(),
        token_program.clone(),
      ],
      &[signers_seeds],
    )?;

    // Finally closing escrow account
    **initializer_main_account.lamports.borrow_mut() = initializer_main_account
      .lamports()
      .checked_add(escrow_account.lamports())
      .ok_or(error::EscrowError::AmountOverflow)?;
    **escrow_account.lamports.borrow_mut() = 0;
    Ok(())
  }
}
