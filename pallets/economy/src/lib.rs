// This file is part of Metaverse.Network & Bit.Country.

// Copyright (C) 2020-2022 Metaverse.Network & Bit.Country .
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Encode, HasCompact};

use frame_support::{
	ensure,
	pallet_prelude::*,
	traits::{Currency, LockableCurrency, ReservableCurrency},
	transactional, PalletId,
};
use frame_system::{ensure_signed, pallet_prelude::*};
use orml_traits::{DataProvider, MultiCurrency, MultiReservableCurrency};
use sp_core::U256;
use sp_runtime::traits::{
	BlockNumberProvider, CheckedAdd, CheckedDiv, CheckedMul, CheckedSub, Saturating, UniqueSaturatedInto,
};
use sp_runtime::{
	traits::{AccountIdConversion, One, Zero},
	ArithmeticError, DispatchError, Perbill, SaturatedConversion,
};
use sp_std::{collections::btree_map::BTreeMap, prelude::*, vec::Vec};

use core_primitives::NFTTrait;
use core_primitives::*;
pub use pallet::*;

use primitives::{estate::Estate, EraIndex, EstateId};
use primitives::{Balance, DomainId, FungibleTokenId, PowerAmount, RoundIndex};
pub use weights::WeightInfo;

/// The Reward Pool Info.
#[derive(Clone, Encode, Decode, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct InnovationStakingPoolInfo<Share: HasCompact, Balance: HasCompact, CurrencyId: Ord> {
	/// Total shares amount
	pub total_shares: Share,
	/// Reward infos <reward_currency, (total_reward, total_withdrawn_reward)>
	pub rewards: BTreeMap<CurrencyId, (Balance, Balance)>,
}

impl<Share, Balance, CurrencyId> Default for InnovationStakingPoolInfo<Share, Balance, CurrencyId>
where
	Share: Default + HasCompact,
	Balance: HasCompact,
	CurrencyId: Ord,
{
	fn default() -> Self {
		Self {
			total_shares: Default::default(),
			rewards: BTreeMap::new(),
		}
	}
}

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

pub mod weights;

#[frame_support::pallet]
pub mod pallet {
	use sp_runtime::traits::{CheckedAdd, CheckedSub, Saturating};
	use sp_runtime::ArithmeticError;

	use primitives::{staking::Bond, ClassId, NftId};

	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(PhantomData<T>);

	pub type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
	pub type TokenId = NftId;
	pub type WithdrawnRewards<T> = BTreeMap<FungibleTokenId, BalanceOf<T>>;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// Because this pallet emits events, it depends on the runtime's definition of an event.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// The currency type
		type Currency: LockableCurrency<Self::AccountId, Moment = BlockNumberFor<Self>>
			+ ReservableCurrency<Self::AccountId>;

		/// Multi-fungible token currency
		type FungibleTokenCurrency: MultiReservableCurrency<
			Self::AccountId,
			CurrencyId = FungibleTokenId,
			Balance = BalanceOf<Self>,
		>;

		/// NFT handler
		type NFTHandler: NFTTrait<Self::AccountId, BalanceOf<Self>, ClassId = ClassId, TokenId = TokenId>;

		/// Round handler
		type RoundHandler: RoundTrait<BlockNumberFor<Self>>;

		/// Estate handler
		type EstateHandler: Estate<Self::AccountId>;

		/// Economy treasury fund
		#[pallet::constant]
		type EconomyTreasury: Get<PalletId>;

		/// The currency id of BIT
		#[pallet::constant]
		type MiningCurrencyId: Get<FungibleTokenId>;

		/// The minimum stake required for staking
		#[pallet::constant]
		type MinimumStake: Get<BalanceOf<Self>>;

		/// The maximum estate staked per land unit
		#[pallet::constant]
		type MaximumEstateStake: Get<BalanceOf<Self>>;

		/// The Power Amount per block
		#[pallet::constant]
		type PowerAmountPerBlock: Get<PowerAmount>;

		// Reward payout account
		#[pallet::constant]
		type RewardPayoutAccount: Get<PalletId>;
		/// Weight info
		type WeightInfo: WeightInfo;
	}

	/// BIT to power exchange rate
	#[pallet::storage]
	#[pallet::getter(fn get_bit_power_exchange_rate)]
	pub(super) type BitPowerExchangeRate<T: Config> = StorageValue<_, Balance, ValueQuery>;

	/// Power balance of user
	#[pallet::storage]
	#[pallet::getter(fn get_power_balance)]
	pub type PowerBalance<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, PowerAmount, ValueQuery>;

	/// TBD Accept domain
	#[pallet::storage]
	#[pallet::getter(fn get_accepted_domain)]
	pub type AcceptedDomain<T: Config> = StorageMap<_, Twox64Concat, DomainId, ()>;

	/// Self-staking info
	#[pallet::storage]
	#[pallet::getter(fn get_staking_info)]
	pub type StakingInfo<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, BalanceOf<T>, ValueQuery>;

	/// Estate-staking info
	#[pallet::storage]
	#[pallet::getter(fn get_estate_staking_info)]
	pub type EstateStakingInfo<T: Config> =
		StorageMap<_, Twox64Concat, EstateId, Bond<T::AccountId, BalanceOf<T>>, OptionQuery>;

	/// Self-staking exit queue info
	/// This will keep track of stake exits queue, unstake only allows after 1 round
	#[pallet::storage]
	#[pallet::getter(fn staking_exit_queue)]
	pub type ExitQueue<T: Config> =
		StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Twox64Concat, RoundIndex, BalanceOf<T>, OptionQuery>;

	/// Estate self-staking exit estate queue info
	/// This will keep track of staked estate exits queue, unstake only allows after 1 round
	#[pallet::storage]
	#[pallet::getter(fn estate_staking_exit_queue)]
	pub type EstateExitQueue<T: Config> = StorageNMap<
		_,
		(
			NMapKey<Blake2_128Concat, T::AccountId>,
			NMapKey<Blake2_128Concat, RoundIndex>,
			NMapKey<Blake2_128Concat, EstateId>,
		),
		BalanceOf<T>,
		OptionQuery,
	>;

	/// Total native token locked in this pallet
	#[pallet::storage]
	#[pallet::getter(fn total_stake)]
	type TotalStake<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Total native token locked estate staking pallet
	#[pallet::storage]
	#[pallet::getter(fn total_estate_stake)]
	type TotalEstateStake<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Innovation staking info
	#[pallet::storage]
	#[pallet::getter(fn get_innovation_staking_info)]
	pub type InnovationStakingInfo<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, BalanceOf<T>, ValueQuery>;

	/// Total innovation staking locked in this pallet
	#[pallet::storage]
	#[pallet::getter(fn total_innovation_staking)]
	type TotalInnovationStaking<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Record share amount, reward currency and withdrawn reward amount for
	/// specific `AccountId`
	///
	/// storage_map AccountId => (Share, BTreeMap<CurrencyId, Balance>)
	#[pallet::storage]
	#[pallet::getter(fn shares_and_withdrawn_rewards)]
	pub type SharesAndWithdrawnRewards<T: Config> =
		StorageMap<_, Twox64Concat, T::AccountId, (BalanceOf<T>, WithdrawnRewards<T>), ValueQuery>;

	/// Record reward pool info.
	///
	/// StakingRewardPoolInfo
	#[pallet::storage]
	#[pallet::getter(fn staking_reward_pool_info)]
	pub type StakingRewardPoolInfo<T: Config> =
		StorageValue<_, InnovationStakingPoolInfo<BalanceOf<T>, BalanceOf<T>, FungibleTokenId>, ValueQuery>;

	/// Self-staking exit queue info
	/// This will keep track of stake exits queue, unstake only allows after 1 round
	#[pallet::storage]
	#[pallet::getter(fn innovation_staking_exit_queue)]
	pub type InnovationStakingExitQueue<T: Config> =
		StorageDoubleMap<_, Blake2_128Concat, T::AccountId, Twox64Concat, RoundIndex, BalanceOf<T>, OptionQuery>;

	/// The pending rewards amount accumulated from staking on innovation, pending reward added when
	/// user claim reward or remove shares
	///
	/// PendingRewards: map AccountId => BTreeMap<FungibleTokenId, Balance>
	#[pallet::storage]
	#[pallet::getter(fn pending_multi_rewards)]
	pub type PendingRewardsOfStakingInnovation<T: Config> =
		StorageMap<_, Twox64Concat, T::AccountId, BTreeMap<FungibleTokenId, BalanceOf<T>>, ValueQuery>;

	/// The current era index
	#[pallet::storage]
	#[pallet::getter(fn current_era)]
	pub type CurrentEra<T: Config> = StorageValue<_, EraIndex, ValueQuery>;

	/// The block number of last era updated
	#[pallet::storage]
	#[pallet::getter(fn last_era_updated_block)]
	pub type LastEraUpdatedBlock<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;

	/// The internal of block number between era.
	#[pallet::storage]
	#[pallet::getter(fn update_era_frequency)]
	pub type UpdateEraFrequency<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;

	/// The estimated staking reward rate per era on innovation staking.
	///
	/// EstimatedStakingRewardRatePerEra: value: Rate
	#[pallet::storage]
	pub type EstimatedStakingRewardPerEra<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;
	#[pallet::event]
	#[pallet::generate_deposit(pub (super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Mining resource burned [amount]
		MiningResourceBurned(Balance),
		/// Self staking to economy 101 [staker, amount]
		SelfStakedToEconomy101(T::AccountId, BalanceOf<T>),
		/// Estate staking to economy 101 [staker, estate_id, amount]
		EstateStakedToEconomy101(T::AccountId, EstateId, BalanceOf<T>),
		/// Self staking removed from economy 101 [staker, amount]
		SelfStakingRemovedFromEconomy101(T::AccountId, BalanceOf<T>),
		/// Estate staking remoed from economy 101 [staker, estate_id, amount]
		EstateStakingRemovedFromEconomy101(T::AccountId, EstateId, BalanceOf<T>),
		/// New BIT to Power exchange rate has updated [amount]
		BitPowerExchangeRateUpdated(Balance),
		/// Unstaked amount has been withdrew after it's expired [account, rate]
		UnstakedAmountWithdrew(T::AccountId, BalanceOf<T>),
		/// Set power balance by sudo [account, power_amount]
		SetPowerBalance(T::AccountId, PowerAmount),
		/// Power conversion request has cancelled [(class_id, token_id), account]
		CancelPowerConversionRequest((ClassId, TokenId), T::AccountId),
		/// Innovation Staking [staker, amount]
		StakedInnovation(T::AccountId, BalanceOf<T>),
		/// Unstaked from Innovation [staker, amount]
		UnstakedInnovation(T::AccountId, BalanceOf<T>),
		/// Claim rewards
		ClaimRewards(T::AccountId, FungibleTokenId, BalanceOf<T>),
		/// Current innovation staking era updated
		CurrentInnovationStakingEraUpdated(EraIndex),
		/// Innovation Staking Era frequency updated
		UpdatedInnovationStakingEraFrequency(BlockNumberFor<T>),
		/// Last innovation staking era updated
		LastInnovationStakingEraUpdated(BlockNumberFor<T>),
		/// Estimated reward per era
		EstimatedRewardPerEraUpdated(BalanceOf<T>),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// NFT asset does not exist
		NFTAssetDoesNotExist,
		/// NFT class does not exist
		NFTClassDoesNotExist,
		/// NFT collection does not exist
		NFTCollectionDoesNotExist,
		/// No permission
		NoPermission,
		/// No authorization
		NoAuthorization,
		/// Insufficient power balance
		AccountHasNoPowerBalance,
		/// Power amount is zero
		PowerAmountIsZero,
		/// Not enough free balance for staking
		InsufficientBalanceForStaking,
		/// Unstake amount greater than staked amount
		UnstakeAmountExceedStakedAmount,
		/// Has scheduled exit staking, only stake after queue exit
		ExitQueueAlreadyScheduled,
		/// Stake amount below minimum staking required
		StakeBelowMinimum,
		/// Withdraw future round
		WithdrawFutureRound,
		/// Exit queue does not exist
		ExitQueueDoesNotExit,
		/// Unstaked amount is zero
		UnstakeAmountIsZero,
		/// Request already exists
		RequestAlreadyExist,
		/// Order has not reach target
		NotReadyToExecute,
		/// Staker is not estate owner
		StakerNotEstateOwner,
		/// Staking estate does not exist
		StakeEstateDoesNotExist,
		/// Stake is not previous owner
		StakerNotPreviousOwner,
		/// No funds staked at estate
		NoFundsStakedAtEstate,
		/// Previous owner still stakes at estate
		PreviousOwnerStillStakesAtEstate,
		/// Has scheduled exit estate staking, only stake after queue exit
		EstateExitQueueAlreadyScheduled,
		/// Estate exit queue does not exist
		EstateExitQueueDoesNotExit,
		/// Stake amount exceed estate max amount
		StakeAmountExceedMaximumAmount,
		/// Invalid era set up config
		InvalidLastEraUpdatedBlock,
		/// Unexpected error
		Unexpected,
		/// Reward pool does not exist
		RewardPoolDoesNotExist,
		/// Invalid reward set up
		InvalidEstimatedRewardSetup,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
			let era_number = Self::get_era_index(<frame_system::Pallet<T>>::block_number());

			if !era_number.is_zero() {
				let _ = Self::update_current_era(era_number).map_err(|err| err).ok();
			}

			T::WeightInfo::stake_b()
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Stake native token to staking ledger to receive build material every round
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// `amount`: the stake amount
		///
		/// Emit `SelfStakedToEconomy101` event or `EstateStakedToEconomy101` event if successful
		#[pallet::weight(
			if estate.is_some() {
				T::WeightInfo::stake_b()
			} else {
				T::WeightInfo::stake_a()
			}
		)]
		#[transactional]
		pub fn stake(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			estate: Option<EstateId>,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			// Check if user has enough balance for staking
			ensure!(
				T::Currency::free_balance(&who) >= amount,
				Error::<T>::InsufficientBalanceForStaking
			);

			let current_round = T::RoundHandler::get_current_round_info();
			match estate {
				None => {
					// Check if user already in exit queue
					ensure!(
						!ExitQueue::<T>::contains_key(&who, current_round.current),
						Error::<T>::ExitQueueAlreadyScheduled
					);

					let staked_balance = StakingInfo::<T>::get(&who);
					let total = staked_balance.checked_add(&amount).ok_or(ArithmeticError::Overflow)?;

					ensure!(total >= T::MinimumStake::get(), Error::<T>::StakeBelowMinimum);

					T::Currency::reserve(&who, amount)?;

					StakingInfo::<T>::insert(&who, total);

					let new_total_staked = TotalStake::<T>::get().saturating_add(amount);
					<TotalStake<T>>::put(new_total_staked);

					Self::deposit_event(Event::SelfStakedToEconomy101(who, amount));
				}
				Some(estate_id) => {
					// Check if user already in exit queue
					ensure!(
						!EstateExitQueue::<T>::contains_key((&who, current_round.current, estate_id)),
						Error::<T>::EstateExitQueueAlreadyScheduled
					);

					ensure!(
						T::EstateHandler::check_estate(estate_id.clone())?,
						Error::<T>::StakeEstateDoesNotExist
					);
					ensure!(
						T::EstateHandler::check_estate_ownership(who.clone(), estate_id.clone())?,
						Error::<T>::StakerNotEstateOwner
					);

					let mut staked_balance: BalanceOf<T> = Zero::zero();
					let staking_bond_value = EstateStakingInfo::<T>::get(estate_id);
					match staking_bond_value {
						Some(staking_bond) => {
							ensure!(
								staking_bond.staker == who.clone(),
								Error::<T>::PreviousOwnerStillStakesAtEstate
							);
							staked_balance = staking_bond.amount;
						}
						_ => {}
					}

					let total = staked_balance.checked_add(&amount).ok_or(ArithmeticError::Overflow)?;

					ensure!(total >= T::MinimumStake::get(), Error::<T>::StakeBelowMinimum);

					// Ensure stake amount less than maximum
					let total_land_units = T::EstateHandler::get_total_land_units(Some(estate_id));
					ensure!(total_land_units > 0, Error::<T>::StakeEstateDoesNotExist);

					let stake_allowance = T::MaximumEstateStake::get()
						.saturating_mul(TryInto::<BalanceOf<T>>::try_into(total_land_units).unwrap_or_default());
					ensure!(total <= stake_allowance, Error::<T>::StakeAmountExceedMaximumAmount);

					T::Currency::reserve(&who, amount)?;

					let new_staking_bond = Bond {
						staker: who.clone(),
						amount: total,
					};

					EstateStakingInfo::<T>::insert(&estate_id, new_staking_bond);

					let new_total_staked = TotalEstateStake::<T>::get().saturating_add(amount);
					<TotalEstateStake<T>>::put(new_total_staked);

					Self::deposit_event(Event::EstateStakedToEconomy101(who, estate_id, amount));
				}
			}

			Ok(().into())
		}

		/// Stake native token to innovation staking ledger to receive reward and voting points
		/// every round
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// `amount`: the stake amount
		///
		/// Emit `SelfStakedToEconomy101` event or `EstateStakedToEconomy101` event if successful
		#[pallet::weight(T::WeightInfo::stake_on_innovation())]
		#[transactional]
		pub fn stake_on_innovation(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;

			// Check if user has enough balance for staking
			ensure!(
				T::Currency::free_balance(&who) >= amount,
				Error::<T>::InsufficientBalanceForStaking
			);

			ensure!(
				!amount.is_zero() || amount >= T::MinimumStake::get(),
				Error::<T>::StakeBelowMinimum
			);

			let current_round = T::RoundHandler::get_current_round_info();

			// Check if user already in exit queue
			ensure!(
				!InnovationStakingExitQueue::<T>::contains_key(&who, current_round.current),
				Error::<T>::ExitQueueAlreadyScheduled
			);

			let staked_balance = InnovationStakingInfo::<T>::get(&who);
			let total = staked_balance.checked_add(&amount).ok_or(ArithmeticError::Overflow)?;

			ensure!(total >= T::MinimumStake::get(), Error::<T>::StakeBelowMinimum);

			T::Currency::reserve(&who, amount)?;

			InnovationStakingInfo::<T>::insert(&who, total);

			let new_total_staked = TotalInnovationStaking::<T>::get().saturating_add(amount);
			<TotalInnovationStaking<T>>::put(new_total_staked);

			Self::add_share(&who, amount);

			Self::deposit_event(Event::StakedInnovation(who, amount));

			Ok(())
		}

		/// Unstake native token to innovation staking ledger to receive reward and voting points
		/// every round
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// `amount`: the unstake amount
		///
		/// Emit `UnstakedInnovation` event if successful
		#[pallet::weight(T::WeightInfo::unstake_on_innovation())]
		#[transactional]
		pub fn unstake_on_innovation(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;

			let staked_balance = InnovationStakingInfo::<T>::get(&who);
			ensure!(amount <= staked_balance, Error::<T>::UnstakeAmountExceedStakedAmount);

			let remaining = staked_balance.checked_sub(&amount).ok_or(ArithmeticError::Underflow)?;

			let amount_to_unstake = if remaining < T::MinimumStake::get() {
				// Remaining amount below minimum, remove all staked amount
				staked_balance
			} else {
				amount
			};

			let current_round = T::RoundHandler::get_current_round_info();
			let next_round = current_round.current.saturating_add(28u32);

			// Check if user already in exit queue of the current
			ensure!(
				!InnovationStakingExitQueue::<T>::contains_key(&who, next_round),
				Error::<T>::ExitQueueAlreadyScheduled
			);

			// This exit queue will be executed by exit_staking extrinsics to unreserved token
			InnovationStakingExitQueue::<T>::insert(&who, next_round.clone(), amount_to_unstake);

			// Update staking info of user immediately
			// Remove staking info
			if amount_to_unstake == staked_balance {
				InnovationStakingInfo::<T>::remove(&who);
			} else {
				InnovationStakingInfo::<T>::insert(&who, remaining);
			}

			let new_total_staked = TotalInnovationStaking::<T>::get().saturating_sub(amount_to_unstake);
			<TotalInnovationStaking<T>>::put(new_total_staked);

			Self::remove_share(&who, amount_to_unstake);

			Self::deposit_event(Event::UnstakedInnovation(who, amount));
			Ok(())
		}

		/// Claim reward from innovation staking ledger to receive reward and voting points
		/// every round
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		///
		/// Emit `ClaimRewards` event if successful
		#[pallet::weight(T::WeightInfo::claim_reward())]
		#[transactional]
		pub fn claim_reward(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;

			Self::claim_rewards(&who);

			PendingRewardsOfStakingInnovation::<T>::mutate_exists(&who, |maybe_pending_multi_rewards| {
				if let Some(pending_multi_rewards) = maybe_pending_multi_rewards {
					for (currency_id, pending_reward) in pending_multi_rewards.iter_mut() {
						if pending_reward.is_zero() {
							continue;
						}

						let payout_amount = pending_reward.clone();

						match Self::distribute_reward(&who, *currency_id, payout_amount) {
							Ok(_) => {
								// update state
								*pending_reward = Zero::zero();

								Self::deposit_event(Event::ClaimRewards(
									who.clone(),
									FungibleTokenId::NativeToken(0),
									payout_amount,
								));
							}
							Err(e) => {
								log::error!(
									target: "economy",
									"staking_payout_reward: failed to payout {:?} to {:?} to {:?}",
									pending_reward, who, e
								);
							}
						}
					}
				}
			});

			Ok(())
		}

		/// Unstake native token from staking ledger. The unstaked amount able to redeem from the
		/// next round
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// `amount`: the stake amount
		///
		/// Emit `SelfStakingRemovedFromEconomy101` event or `EstateStakingRemovedFromEconomy101`
		/// event if successful
		#[pallet::weight(
			if estate.is_some() {
				T::WeightInfo::unstake_b()
			} else {
				T::WeightInfo::unstake_a()
			}
		)]
		pub fn unstake(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			estate: Option<EstateId>,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			// Ensure amount is greater than zero
			ensure!(!amount.is_zero(), Error::<T>::UnstakeAmountIsZero);

			match estate {
				None => {
					let staked_balance = StakingInfo::<T>::get(&who);
					ensure!(amount <= staked_balance, Error::<T>::UnstakeAmountExceedStakedAmount);

					let remaining = staked_balance.checked_sub(&amount).ok_or(ArithmeticError::Underflow)?;

					let amount_to_unstake = if remaining < T::MinimumStake::get() {
						// Remaining amount below minimum, remove all staked amount
						staked_balance
					} else {
						amount
					};

					let current_round = T::RoundHandler::get_current_round_info();
					let next_round = current_round.current.saturating_add(One::one());

					// Check if user already in exit queue of the current
					ensure!(
						!ExitQueue::<T>::contains_key(&who, next_round),
						Error::<T>::ExitQueueAlreadyScheduled
					);

					// This exit queue will be executed by exit_staking extrinsics to unreserved token
					ExitQueue::<T>::insert(&who, next_round.clone(), amount_to_unstake);

					// Update staking info of user immediately
					// Remove staking info
					if amount_to_unstake == staked_balance {
						StakingInfo::<T>::remove(&who);
					} else {
						StakingInfo::<T>::insert(&who, remaining);
					}

					let new_total_staked = TotalStake::<T>::get().saturating_sub(amount_to_unstake);
					<TotalStake<T>>::put(new_total_staked);

					Self::deposit_event(Event::SelfStakingRemovedFromEconomy101(who, amount));
				}
				Some(estate_id) => {
					ensure!(
						T::EstateHandler::check_estate(estate_id.clone())?,
						Error::<T>::StakeEstateDoesNotExist
					);

					let mut staked_balance = Zero::zero();
					let staking_bond_value = EstateStakingInfo::<T>::get(estate_id);
					match staking_bond_value {
						Some(staking_bond) => {
							ensure!(staking_bond.staker == who.clone(), Error::<T>::NoFundsStakedAtEstate);
							staked_balance = staking_bond.amount;
						}
						_ => {}
					}
					ensure!(amount <= staked_balance, Error::<T>::UnstakeAmountExceedStakedAmount);

					let remaining = staked_balance.checked_sub(&amount).ok_or(ArithmeticError::Underflow)?;

					let amount_to_unstake = if remaining < T::MinimumStake::get() {
						// Remaining amount below minimum, remove all staked amount
						staked_balance
					} else {
						amount
					};

					let current_round = T::RoundHandler::get_current_round_info();
					let next_round = current_round.current.saturating_add(One::one());

					// Check if user already in estate exit queue of the current estate
					ensure!(
						!EstateExitQueue::<T>::contains_key((&who, next_round, estate_id)),
						Error::<T>::ExitQueueAlreadyScheduled
					);

					// This estate exit queue will be executed by exit_staking extrinsics to unreserved token
					EstateExitQueue::<T>::insert((&who, next_round.clone(), estate_id), amount_to_unstake);

					// Update estate staking info of user immediately
					// Remove estate staking info
					if amount_to_unstake == staked_balance {
						EstateStakingInfo::<T>::remove(&estate_id);
					} else {
						let new_staking_bond = Bond {
							staker: who.clone(),
							amount: remaining,
						};
						EstateStakingInfo::<T>::insert(&estate_id, new_staking_bond);
					}

					let new_total_staked = TotalEstateStake::<T>::get().saturating_sub(amount_to_unstake);
					<TotalEstateStake<T>>::put(new_total_staked);

					Self::deposit_event(Event::EstateStakingRemovedFromEconomy101(who, estate_id, amount));
				}
			}

			Ok(().into())
		}

		/// Unstake native token (staked by previous owner) from staking ledger.
		///
		/// The dispatch origin for this call must be _Signed_. Works if the origin is the estate
		/// owner and the previous owner got staked funds
		///
		/// `estate_id`: the estate ID which funds are going to be unstaked
		///
		/// Emit `EstateStakingRemovedFromEconomy101` event if successful
		#[pallet::weight(T::WeightInfo::unstake_new_estate_owner())]
		pub fn unstake_new_estate_owner(origin: OriginFor<T>, estate_id: EstateId) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			ensure!(
				T::EstateHandler::check_estate(estate_id.clone())?,
				Error::<T>::StakeEstateDoesNotExist
			);

			ensure!(
				T::EstateHandler::check_estate_ownership(who.clone(), estate_id.clone())?,
				Error::<T>::StakerNotEstateOwner
			);

			let staking_bond_value = EstateStakingInfo::<T>::get(estate_id);
			match staking_bond_value {
				Some(staking_info) => {
					ensure!(
						staking_info.staker.clone() != who.clone(),
						Error::<T>::StakerNotPreviousOwner
					);
					let staked_balance = staking_info.amount;

					let current_round = T::RoundHandler::get_current_round_info();
					let next_round = current_round.current.saturating_add(One::one());

					// This exit queue will be executed by exit_staking extrinsics to unreserved token
					EstateExitQueue::<T>::insert((&staking_info.staker, next_round.clone(), estate_id), staked_balance);
					EstateStakingInfo::<T>::remove(&estate_id);

					let new_total_staked = TotalEstateStake::<T>::get().saturating_sub(staked_balance);
					<TotalEstateStake<T>>::put(new_total_staked);

					Self::deposit_event(Event::EstateStakingRemovedFromEconomy101(
						who,
						estate_id,
						staked_balance,
					));
					Ok(().into())
				}
				None => Err(Error::<T>::StakeEstateDoesNotExist.into()),
			}
		}

		/// Withdraw unstaked token from unstaking queue. The unstaked amount will be unreserved and
		/// become transferrable
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// `round_index`: the round index that user can unstake.
		///
		/// Emit `UnstakedAmountWithdrew` event if successful
		#[pallet::weight(T::WeightInfo::withdraw_unreserved())]
		pub fn withdraw_unreserved(origin: OriginFor<T>, round_index: RoundIndex) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			// Get user exit queue
			let exit_balance = ExitQueue::<T>::get(&who, round_index).ok_or(Error::<T>::ExitQueueDoesNotExit)?;

			ExitQueue::<T>::remove(&who, round_index);
			T::Currency::unreserve(&who, exit_balance);

			Self::deposit_event(Event::<T>::UnstakedAmountWithdrew(who, exit_balance));

			Ok(().into())
		}

		/// Withdraw unstaked token from estate unstaking queue. The unstaked amount will be
		/// unreserved and become transferrable
		///
		/// The dispatch origin for this call must be _Signed_.
		///
		/// `round_index`: the round index that user can redeem.
		/// `estate_id`: the estate id that user can redeem.
		///
		/// Emit `UnstakedAmountWithdrew` event if successful
		#[pallet::weight(T::WeightInfo::withdraw_unreserved())]
		pub fn withdraw_estate_unreserved(
			origin: OriginFor<T>,
			round_index: RoundIndex,
			estate_id: EstateId,
		) -> DispatchResultWithPostInfo {
			let who = ensure_signed(origin)?;

			// Get user exit queue
			let exit_balance = EstateExitQueue::<T>::get((&who, round_index, estate_id))
				.ok_or(Error::<T>::EstateExitQueueDoesNotExit)?;

			EstateExitQueue::<T>::remove((&who, round_index, estate_id));
			T::Currency::unreserve(&who, exit_balance);

			Self::deposit_event(Event::<T>::UnstakedAmountWithdrew(who, exit_balance));

			Ok(().into())
		}

		/// Force unstake native token from staking ledger. The unstaked amount able to redeem
		/// immediately
		///
		///
		/// The dispatch origin for this call must be _Root_.
		///
		/// `amount`: the stake amount
		/// `who`: the address of staker
		///
		/// Emit `SelfStakingRemovedFromEconomy101` event or `EstateStakingRemovedFromEconomy101`
		/// event if successful
		#[pallet::weight(
			if estate.is_some() {
				T::WeightInfo::unstake_b()
			} else {
				T::WeightInfo::unstake_a()
			}
		)]
		pub fn force_unstake(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			who: T::AccountId,
			estate: Option<EstateId>,
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;

			// Ensure amount is greater than zero
			ensure!(!amount.is_zero(), Error::<T>::UnstakeAmountIsZero);

			match estate {
				None => {
					let staked_balance = StakingInfo::<T>::get(&who);
					ensure!(amount <= staked_balance, Error::<T>::UnstakeAmountExceedStakedAmount);

					let remaining = staked_balance.checked_sub(&amount).ok_or(ArithmeticError::Underflow)?;

					let amount_to_unstake = if remaining < T::MinimumStake::get() {
						// Remaining amount below minimum, remove all staked amount
						staked_balance
					} else {
						amount
					};

					// Update staking info of user immediately
					// Remove staking info
					if amount_to_unstake == staked_balance {
						StakingInfo::<T>::remove(&who);
					} else {
						StakingInfo::<T>::insert(&who, remaining);
					}

					let new_total_staked = TotalStake::<T>::get().saturating_sub(amount_to_unstake);
					<TotalStake<T>>::put(new_total_staked);

					T::Currency::unreserve(&who, amount_to_unstake);

					Self::deposit_event(Event::UnstakedAmountWithdrew(who.clone(), amount_to_unstake));
					Self::deposit_event(Event::SelfStakingRemovedFromEconomy101(who, amount));
				}
				Some(estate_id) => {
					ensure!(
						T::EstateHandler::check_estate(estate_id.clone())?,
						Error::<T>::StakeEstateDoesNotExist
					);
					let mut staked_balance: BalanceOf<T> = Zero::zero();
					let staking_bond_value = EstateStakingInfo::<T>::get(estate_id);
					match staking_bond_value {
						Some(staking_bond) => {
							ensure!(staking_bond.staker == who.clone(), Error::<T>::NoFundsStakedAtEstate);
							staked_balance = staking_bond.amount;
						}
						_ => {}
					}
					ensure!(amount <= staked_balance, Error::<T>::UnstakeAmountExceedStakedAmount);

					let remaining = staked_balance.checked_sub(&amount).ok_or(ArithmeticError::Underflow)?;

					let amount_to_unstake = if remaining < T::MinimumStake::get() {
						// Remaining amount below minimum, remove all staked amount
						staked_balance
					} else {
						amount
					};

					// Update staking info of user immediately
					// Remove staking info
					if amount_to_unstake == staked_balance {
						EstateStakingInfo::<T>::remove(&estate_id);
					} else {
						let new_staking_bond = Bond {
							staker: who.clone(),
							amount: remaining,
						};
						EstateStakingInfo::<T>::insert(&estate_id, new_staking_bond);
					}

					let new_total_staked = TotalStake::<T>::get().saturating_sub(amount_to_unstake);
					<TotalEstateStake<T>>::put(new_total_staked);

					T::Currency::unreserve(&who, amount_to_unstake);

					Self::deposit_event(Event::UnstakedAmountWithdrew(who.clone(), amount_to_unstake));
					Self::deposit_event(Event::EstateStakingRemovedFromEconomy101(who, estate_id, amount));
				}
			}
			Ok(().into())
		}

		/// Force unreserved unstake native token from staking ledger. The unstaked amount able to
		/// unreserve immediately
		///
		///
		/// The dispatch origin for this call must be _Root_.
		///
		/// `amount`: the stake amount
		/// `who`: the address of staker
		///
		/// Emit `SelfStakingRemovedFromEconomy101` event if successful
		#[pallet::weight(T::WeightInfo::unstake_b())]
		pub fn force_unreserved_staking(
			origin: OriginFor<T>,
			amount: BalanceOf<T>,
			who: T::AccountId,
		) -> DispatchResultWithPostInfo {
			ensure_root(origin)?;

			// Ensure amount is greater than zero
			ensure!(!amount.is_zero(), Error::<T>::UnstakeAmountIsZero);

			// Update staking info
			let staked_reserved_balance = T::Currency::reserved_balance(&who);
			ensure!(
				amount <= staked_reserved_balance,
				Error::<T>::UnstakeAmountExceedStakedAmount
			);

			T::Currency::unreserve(&who, amount);

			Ok(().into())
		}

		/// This function only for governance origin to execute when starting the protocol or
		/// changes of era duration.
		#[pallet::weight(< T as Config >::WeightInfo::stake_b())]
		pub fn update_era_config(
			origin: OriginFor<T>,
			last_era_updated_block: Option<BlockNumberFor<T>>,
			frequency: Option<BlockNumberFor<T>>,
			estimated_reward_rate_per_era: Option<BalanceOf<T>>,
		) -> DispatchResult {
			let _ = ensure_root(origin)?;

			if let Some(change) = frequency {
				UpdateEraFrequency::<T>::put(change);
				Self::deposit_event(Event::<T>::UpdatedInnovationStakingEraFrequency(change));
			}

			if let Some(change) = last_era_updated_block {
				let update_era_frequency = UpdateEraFrequency::<T>::get();
				let current_block = <frame_system::Pallet<T>>::block_number();
				if !update_era_frequency.is_zero() {
					ensure!(
						change > current_block.saturating_sub(update_era_frequency) && change <= current_block,
						Error::<T>::InvalidLastEraUpdatedBlock
					);

					LastEraUpdatedBlock::<T>::put(change);
					Self::deposit_event(Event::<T>::LastInnovationStakingEraUpdated(change));
				}
			}

			if let Some(reward_rate_per_era) = estimated_reward_rate_per_era {
				EstimatedStakingRewardPerEra::<T>::put(reward_rate_per_era);
				Self::deposit_event(Event::<T>::EstimatedRewardPerEraUpdated(reward_rate_per_era));
			}
			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	pub fn economy_pallet_account_id() -> T::AccountId {
		T::EconomyTreasury::get().into_account_truncating()
	}

	pub fn convert_power_to_bit(power_amount: Balance, commission: Perbill) -> (Balance, Balance) {
		let rate = Self::get_bit_power_exchange_rate();

		let bit_required = power_amount
			.checked_mul(rate)
			.ok_or(ArithmeticError::Overflow)
			.unwrap_or(Zero::zero());
		let commission_fee = commission * bit_required;
		(
			bit_required + commission_fee,
			TryInto::<Balance>::try_into(commission_fee).unwrap_or_default(),
		)
	}

	fn do_burn(_who: &T::AccountId, amount: Balance) -> DispatchResult {
		if amount.is_zero() {
			return Ok(());
		}

		Self::deposit_event(Event::<T>::MiningResourceBurned(amount));

		Ok(())
	}

	fn distribute_power_by_network(power_amount: PowerAmount, beneficiary: &T::AccountId) -> DispatchResult {
		let mut distributor_power_balance = PowerBalance::<T>::get(beneficiary);
		distributor_power_balance = distributor_power_balance
			.checked_add(power_amount)
			.ok_or(ArithmeticError::Overflow)?;

		PowerBalance::<T>::insert(beneficiary.clone(), power_amount);

		Ok(())
	}

	fn get_target_execution_order(power_amount: PowerAmount) -> Result<BlockNumberFor<T>, DispatchError> {
		let current_block_number = <frame_system::Pallet<T>>::current_block_number();
		let target_block = if power_amount <= T::PowerAmountPerBlock::get() {
			let target_b = current_block_number
				.checked_add(&One::one())
				.ok_or(ArithmeticError::Overflow)?;
			target_b
		} else {
			let block_required = power_amount
				.checked_div(T::PowerAmountPerBlock::get())
				.ok_or(ArithmeticError::Overflow)?;

			let target_b = current_block_number
				.checked_add(&TryInto::<BlockNumberFor<T>>::try_into(block_required).unwrap_or_default())
				.ok_or(ArithmeticError::Overflow)?;
			target_b
		};

		Ok(target_block)
	}

	fn check_target_execution(target: BlockNumberFor<T>) -> bool {
		let current_block_number = <frame_system::Pallet<T>>::current_block_number();

		current_block_number >= target
	}

	pub fn add_share(who: &T::AccountId, add_amount: BalanceOf<T>) {
		if add_amount.is_zero() {
			return;
		}

		StakingRewardPoolInfo::<T>::mutate(|pool_info| {
			let initial_total_shares = pool_info.total_shares;
			pool_info.total_shares = pool_info.total_shares.saturating_add(add_amount);

			let mut withdrawn_inflation = Vec::<(FungibleTokenId, BalanceOf<T>)>::new();

			pool_info
				.rewards
				.iter_mut()
				.for_each(|(reward_currency, (total_reward, total_withdrawn_reward))| {
					let reward_inflation = if initial_total_shares.is_zero() {
						Zero::zero()
					} else {
						U256::from(add_amount.to_owned().saturated_into::<u128>())
							.saturating_mul(total_reward.to_owned().saturated_into::<u128>().into())
							.checked_div(initial_total_shares.to_owned().saturated_into::<u128>().into())
							.unwrap_or_default()
							.as_u128()
							.saturated_into()
					};
					*total_reward = total_reward.saturating_add(reward_inflation);
					*total_withdrawn_reward = total_withdrawn_reward.saturating_add(reward_inflation);

					withdrawn_inflation.push((*reward_currency, reward_inflation));
				});

			SharesAndWithdrawnRewards::<T>::mutate(who, |(share, withdrawn_rewards)| {
				*share = share.saturating_add(add_amount);
				// update withdrawn inflation for each reward currency
				withdrawn_inflation
					.into_iter()
					.for_each(|(reward_currency, reward_inflation)| {
						withdrawn_rewards
							.entry(reward_currency)
							.and_modify(|withdrawn_reward| {
								*withdrawn_reward = withdrawn_reward.saturating_add(reward_inflation);
							})
							.or_insert(reward_inflation);
					});
			});
		});
	}

	pub fn remove_share(who: &T::AccountId, remove_amount: BalanceOf<T>) {
		if remove_amount.is_zero() {
			return;
		}

		// claim rewards firstly
		Self::claim_rewards(who);

		SharesAndWithdrawnRewards::<T>::mutate_exists(who, |share_info| {
			if let Some((mut share, mut withdrawn_rewards)) = share_info.take() {
				let remove_amount = remove_amount.min(share);

				if remove_amount.is_zero() {
					return;
				}

				StakingRewardPoolInfo::<T>::mutate_exists(|maybe_pool_info| {
					if let Some(mut pool_info) = maybe_pool_info.take() {
						let removing_share = U256::from(remove_amount.saturated_into::<u128>());

						pool_info.total_shares = pool_info.total_shares.saturating_sub(remove_amount);

						// update withdrawn rewards for each reward currency
						withdrawn_rewards
							.iter_mut()
							.for_each(|(reward_currency, withdrawn_reward)| {
								let withdrawn_reward_to_remove: BalanceOf<T> = removing_share
									.saturating_mul(withdrawn_reward.to_owned().saturated_into::<u128>().into())
									.checked_div(share.saturated_into::<u128>().into())
									.unwrap_or_default()
									.as_u128()
									.saturated_into();

								if let Some((total_reward, total_withdrawn_reward)) =
									pool_info.rewards.get_mut(reward_currency)
								{
									*total_reward = total_reward.saturating_sub(withdrawn_reward_to_remove);
									*total_withdrawn_reward =
										total_withdrawn_reward.saturating_sub(withdrawn_reward_to_remove);

									// remove if all reward is withdrawn
									if total_reward.is_zero() {
										pool_info.rewards.remove(reward_currency);
									}
								}
								*withdrawn_reward = withdrawn_reward.saturating_sub(withdrawn_reward_to_remove);
							});

						if !pool_info.total_shares.is_zero() {
							*maybe_pool_info = Some(pool_info);
						}
					}
				});

				share = share.saturating_sub(remove_amount);
				if !share.is_zero() {
					*share_info = Some((share, withdrawn_rewards));
				}
			}
		});
	}

	pub fn claim_rewards(who: &T::AccountId) {
		SharesAndWithdrawnRewards::<T>::mutate_exists(who, |maybe_share_withdrawn| {
			if let Some((share, withdrawn_rewards)) = maybe_share_withdrawn {
				if share.is_zero() {
					return;
				}

				StakingRewardPoolInfo::<T>::mutate_exists(|maybe_pool_info| {
					if let Some(pool_info) = maybe_pool_info {
						let total_shares = U256::from(pool_info.total_shares.to_owned().saturated_into::<u128>());
						pool_info.rewards.iter_mut().for_each(
							|(reward_currency, (total_reward, total_withdrawn_reward))| {
								Self::claim_one(
									withdrawn_rewards,
									*reward_currency,
									share.to_owned(),
									total_reward.to_owned(),
									total_shares,
									total_withdrawn_reward,
									who,
								);
							},
						);
					}
				});
			}
		});
	}

	#[allow(clippy::too_many_arguments)] // just we need to have all these to do the stuff
	fn claim_one(
		withdrawn_rewards: &mut BTreeMap<FungibleTokenId, BalanceOf<T>>,
		reward_currency: FungibleTokenId,
		share: BalanceOf<T>,
		total_reward: BalanceOf<T>,
		total_shares: U256,
		total_withdrawn_reward: &mut BalanceOf<T>,
		who: &T::AccountId,
	) {
		let withdrawn_reward = withdrawn_rewards.get(&reward_currency).copied().unwrap_or_default();
		let reward_to_withdraw = Self::reward_to_withdraw(
			share,
			total_reward,
			total_shares,
			withdrawn_reward,
			total_withdrawn_reward.to_owned(),
		);
		if !reward_to_withdraw.is_zero() {
			*total_withdrawn_reward = total_withdrawn_reward.saturating_add(reward_to_withdraw);
			withdrawn_rewards.insert(reward_currency, withdrawn_reward.saturating_add(reward_to_withdraw));

			// pay reward to `who`
			Self::reward_payout(who, reward_currency, reward_to_withdraw);
		}
	}

	fn reward_to_withdraw(
		share: BalanceOf<T>,
		total_reward: BalanceOf<T>,
		total_shares: U256,
		withdrawn_reward: BalanceOf<T>,
		total_withdrawn_reward: BalanceOf<T>,
	) -> BalanceOf<T> {
		let total_reward_proportion: BalanceOf<T> = U256::from(share.saturated_into::<u128>())
			.saturating_mul(U256::from(total_reward.saturated_into::<u128>()))
			.checked_div(total_shares)
			.unwrap_or_default()
			.as_u128()
			.unique_saturated_into();
		total_reward_proportion
			.saturating_sub(withdrawn_reward)
			.min(total_reward.saturating_sub(total_withdrawn_reward))
	}

	fn reward_payout(who: &T::AccountId, currency_id: FungibleTokenId, payout_amount: BalanceOf<T>) {
		if payout_amount.is_zero() {
			return;
		}
		PendingRewardsOfStakingInnovation::<T>::mutate(who, |rewards| {
			rewards
				.entry(currency_id)
				.and_modify(|current| *current = current.saturating_add(payout_amount))
				.or_insert(payout_amount);
		});
	}

	/// Ensure atomic
	#[transactional]
	fn distribute_reward(
		who: &T::AccountId,
		reward_currency_id: FungibleTokenId,
		payout_amount: BalanceOf<T>,
	) -> DispatchResult {
		T::FungibleTokenCurrency::transfer(
			reward_currency_id,
			&Self::get_reward_payout_account_id(),
			who,
			payout_amount,
		)?;
		Ok(())
	}

	pub fn get_reward_payout_account_id() -> T::AccountId {
		T::RewardPayoutAccount::get().into_account_truncating()
	}

	pub fn get_era_index(block_number: BlockNumberFor<T>) -> EraIndex {
		block_number
			.checked_sub(&Self::last_era_updated_block())
			.and_then(|n| n.checked_div(&Self::update_era_frequency()))
			.and_then(|n| TryInto::<EraIndex>::try_into(n).ok())
			.unwrap_or_else(Zero::zero)
	}

	#[transactional]
	pub fn update_current_era(era_index: EraIndex) -> DispatchResult {
		let previous_era = Self::current_era();
		let new_era = previous_era.saturating_add(era_index);

		Self::handle_reward_distribution_to_reward_pool_every_era(previous_era, new_era.clone())?;
		CurrentEra::<T>::put(new_era.clone());
		LastEraUpdatedBlock::<T>::put(<frame_system::Pallet<T>>::block_number());

		Self::deposit_event(Event::<T>::CurrentInnovationStakingEraUpdated(new_era.clone()));
		Ok(())
	}

	fn handle_reward_distribution_to_reward_pool_every_era(
		previous_era: EraIndex,
		new_era: EraIndex,
	) -> DispatchResult {
		let era_changes = new_era.saturating_sub(previous_era);
		ensure!(!era_changes.is_zero(), Error::<T>::Unexpected);
		// Get reward per era that set up Governance
		let reward_per_era = EstimatedStakingRewardPerEra::<T>::get();
		// Get reward holding account
		let reward_holding_origin = T::RewardPayoutAccount::get().into_account_truncating();
		let reward_holding_balance = T::Currency::free_balance(&reward_holding_origin);

		if reward_holding_balance.is_zero() {
			// Ignore if reward distributor balance is zero
			return Ok(());
		}

		let total_reward = reward_per_era.saturating_mul(era_changes.into());
		let mut amount_to_send = total_reward.clone();
		// Make sure user distributor account has enough balance
		if amount_to_send > reward_holding_balance {
			amount_to_send = reward_holding_balance
		}

		Self::accumulate_reward(FungibleTokenId::NativeToken(0), amount_to_send)?;
		Ok(())
	}

	pub fn accumulate_reward(reward_currency: FungibleTokenId, reward_increment: BalanceOf<T>) -> DispatchResult {
		if reward_increment.is_zero() {
			return Ok(());
		}
		StakingRewardPoolInfo::<T>::mutate_exists(|maybe_pool_info| -> DispatchResult {
			let pool_info = maybe_pool_info.as_mut().ok_or(Error::<T>::RewardPoolDoesNotExist)?;

			pool_info
				.rewards
				.entry(reward_currency)
				.and_modify(|(total_reward, _)| {
					*total_reward = total_reward.saturating_add(reward_increment);
				})
				.or_insert((reward_increment, Zero::zero()));

			Ok(())
		})
	}
}
