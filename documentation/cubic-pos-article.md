# Introduction

Namada is a L1 proof-of-stake blockchain that provides interchain asset-agnostic privacy. Namada is based on a byzantine fault-tolerant (BFT) consensus mechanism widely used in the Cosmos ecosystem, provided by Tendermint ([Go implementation]((https://github.com/tendermint/tendermint)) with the [rust bindings]((https://github.com/informalsystems/tendermint-rs)))., that enables fast block finality times and much lower computational overhead than in proof-of-work (PoW) consensus mechanisms, as in Bitcoin. For consensus to be achieved in a BFT system, at least a 2/3 majority of the nodes (called validators) must be honest and behave according to the protocol.

Namada uses proof-of-stake (PoS) to provide security against Sybil attacks on the network, wherein a malicious entity achieves enough voting power to disrupt consensus of the true chain. Rather than miners competing to create the next block by hashing with lots of computing power in PoW systems, the block proposer is selected from a set of validators with probability proportional to the validator's bonded stake (or voting power). This voting power is then used by the validators to determine if the 2/3 majority of consensus has been achieved. Furthermore, the block proposer mechanism also allows PoS chains to be much greener than PoW chains that need to consume massive amounts of energy to reach consensus.

Namada's PoS module is inspired in large part by the PoS system of the Cosmos Hub and by [prior discussions](https://github.com/cosmos/cosmos-sdk/blob/019444ae4328beaca32f2f8416ee5edbac2ef30b/docs/architecture/adr-039-epoched-staking.md#pipelining-the-epochs) on its future improvements, which have not yet been implemented. The ["nothing at stake" problem](https://blog.ethereum.org/2014/01/15/slasher-a-punitive-proof-of-stake-algorithm) is solved by slashing misbehaving validators, which amounts primarily to confiscating their bonded tokens.

In the PoS system, both users and validators are incentivized with rewards to contribute to the security of the network by bonding tokens in the PoS module. Namada has a regular schedule for minting new tokens (inflation), many of which are owed to accounts that have bonded tokens. Non-validating accounts can participate by *delegating* their tokens to a particular validator: the tokens are locked in the PoS system and the target validator can then use those tokens as voting power in the consensus mechanism and governance.

This article is divided into three main sections. First, we describe the mechanics and configuration of the epoched PoS system. Then we describe Namada's novel approach to slashing for validator misbehavior. Finally, we describe the inflation and rewards system within the PoS module.


# Epoched Proof of Stake

The native staking token in Namada is identified as *NAM*. We call the smallest denomination of this token *namnam*, where 1 million *namnam* is equal to 1 *NAM*. An epoch defines a grouped set of consecutive blocks and a period of time that have some fixed configuration of the PoS system. For example, the set of validators who participate in the consensus protocol and their associated consensus voting powers can only change at the boundary of two epochs and are constant throughout the duration of one.

Namada employs epoched staking for two main reasons:
- Light clients become more efficient because there are fewer state changes (particularly validator set changes) to keep track of, resulting in quicker state verification
- Eventually, Namada (and the larger Anoma ecosystem) will rely on the [Ferveo DKG protocol](https://eprint.iacr.org/2022/898) to encrypt transactions in the mempool. Ferveo must know the validator set for a near-future epoch in order to have sufficient time to compute the DKG in advance of the given epoch.

Thus, changes to the state of the PoS system are enqueued to take effect at some epoch in the future, the lengths of which we refer to as *offsets* relative to the epoch in which the transaction is submitted. Most state changes, including validator set updates, take effect at the *pipeline offset* - 2 epochs into the future. The other important offset in the model is the *unbonding offset* of 21 epochs, primarily the length of time before an unbond becomes withdrawable.

**Important parameters in the Namada PoS model (followed by their default values)**
- Duration of a block: ~ 5 - 10 seconds
- Duration of an epoch: ~ 1 day
- Pipeline offset length: 2 epochs
- Unbonding offset length: 21 epochs

Below, we will review the rules around important PoS actions
- Bonding tx submitted at epoch `n` initiates changes at epoch `n + pipeline_offset (n+2)`
	- Bond becomes active, validator bonded stake and validator set is updated
- Unbonding tx submitted at epoch `n`
	- Updates validator set with decreased validator voting power at epoch `n + pipeline_offset (n+2)`
	- Amount of the unbond is withdrawable at epoch `n + unbonding_offset + pipeline_offset (n+23)`
- Redelegation tx submitted at epoch `n`
	- Updates validator set with voting powers for old and new validators at epoch `n + pipeline_offset (n+2)`
	- Only support redelegating 100% of the token amount of a bond
	- A redelegated bond cannot be redelegated again until epoch `n + unbonding_offset + pipeline_offset (n+23)`
	- Slashes are properly handled to ensure that only tokens that contributed to a validator's voting power at the time of infraction are affected

**Validator Sets**

Namada validators are classified into one of three sets:
- **Consensus:** validators that are active in the consensus voting mechanism, chosen to be the `max_consensus_validators` largest validators weighted by their voting power. They are the only validators who can earn block rewards from the PoS inflation.
- **Below capacity:** validators whose bonded stake is less than the *consensus* validators but above some small threshold `min_validator_stake`.
- **Below threshold:** all other validators with stake below `min_validator_stake`.

The *consensus* and *below capacity* validator sets are explicitly held in storage, ordered by their bonded stake, and are updated at every epoch boundary. However, no ordered set of *below threshold* validators is kept in storage. This validator set construction and the `min_validator_stake` threshold exist primarily to prevent the possibility of significant slowing of PoS system updates due to spam validator account creation. Thus, unbounded iteration when updating and validating PoS system state changes is avoided. Conversely, the *below capacity* validator set must be kept in storage, ordered by bonded stake, in order to properly process validator set changes in which validators may drop out or enter the *consensus* validator set. Note that this construction of the validator sets is only important for the consensus mechanism; all validators can participate with their voting power in governance.

**Important validator set configuration values, mutable via governance (with default values):**
- `max_consensus_validators` = 100
- `min_validator_stake` = 1 NAM

# Cubic Slashing

As in many other PoS blockchains, slashing is applied as a way to punish misbehaving validators. If a validator is detected to have misbehaved then portions of the validator's bonded stake will be slashed or seized. This propagates down to the stake of delegators associated to the misbehaving validator as well. Validator infractions that are subject to slashing include:

- signing an invalid block
- signing two different blocks at the same height
- amnesia attacks (once they are supported by tendermint, see [this issue](https://github.com/tendermint/tendermint/issues/5270))

Ultimately, slashed tokens are sent to a slash pool fund, from which the funds can be spent by governance to refund slashed accounts in the event of accidental misbehavior.

Typically, the slash amount for an infraction is proportional to a validator's voting power, however  Namada employs a slashing scheme that has more severe punishments for correlated validator misbehavior: so-called [*cubic slashing*](https://specs.namada.net/economics/proof-of-stake/cubic-slashing.html), wherein the slashed token amount can be proportional to the cube of a validator's voting power (the slash rate is quadratic). Cubic slashing is designed to impose more severe punishments for deliberate, repeated validator attacks but much milder punishments for accidental or non-malicious misbehaviors.

When a validator misbehavior is detected by the protocol, the misbehaving validator is immediately jailed (unable to participate in consensus or make PoS transactions). The slash for the infraction is queued up to be processed at the epoch `n + unbonding_len` for an infraction committed in epoch `n`, allowing the protocol a sufficiently long time period (~21 days) to detect infractions. The cubic slashing algorithm is applied when processing the slash at the epoch `n + unbonding_len` and works as follows:

1. For each slash queued to be processed in epoch `n`, collect the set of known slashes to be processed in epochs \[n-1, n+1\].
2. Sum the fractional voting powers of the validator associated to each slash in this window:

$$\text{sum}~ = \sum_{i \in \text{infractions}} \frac{\text{vp}_{i}}{\text{vp}_{tot}}$$

Note that the voting power in the above expression is the voting power of the validator *when the infraction was committed*.

3. The slash rate is then a function of the square of this sum, bounded below by some configurable minimum slash rate $r_{\text{min}}$ (with the corresponding plot):

$$\max (~r_{\text{min}}~,~9*\text{sum}^2 ~).$$

The factor of $9$ is included such that the slash rate maxes out at $100\%$ for total voting power of $1/3$. This is a critical point in the BFT consensus model, as a total colluding voting power of greater than $1/3$ can prevent the chain from achieving consensus. The cubic slashing rate is also plotted here for visual convenience:

[<img src="cubic_slash.png" width="500"/>](cubic_slash.png)

Once the slash rate is determined, the voting power of the jailed misbehaving validator can be immediately slashed and updated. This validator can then submit a transaction to become unjailed, which takes effect at the pipeline offset relative to the epoch of submission. Once the validator has been reinstated into the appropriate validator set depending on its new voting power, it and its delegators can resume bonding, unbonding, and redelegating.

Namada's slashing system also guarantees that only delegators whose stake were used in a misbehaving validator's voting power during the epoch of infraction are slashed. If a delegator that contributed to a misbehaving validator's voting power at the time of infraction has since redelegated to a new validator by the time the infraction is discovered, then the protocol will still properly slash the delegator. This is an improved PoS guarantee offered by Namada, whereas in other chains using the Cosmos SDK's PoS module, there is no guarantee that all delegators contributing voting power to an infraction (and only these delegators) will be properly slashed for the infraction. For further context, see [this issue](https://github.com/cosmos/cosmos-sdk/issues/1440).

# Inflation and Rewards

The Namada protocol mints inflationary tokens at the beginning of each new epoch, with some of the rewards reserved for those who have locked tokens in the PoS system to be used for consensus voting power.

**Inflation system (PD controller)**

The inflation amount is dynamic, subject to change at each epoch, and is dictated by a [PD controller mechanism](https://electronicscoach.com/proportional-derivative-controller.html). The PD controller adjusts the inflation rate every epoch subject to some maximum rate, and the adjustment is performed to incentivize the achievement of a target ratio of total staked tokens to the total token supply of the protocol. If the staked token ratio is below the target rate during one epoch, then the next epoch the rate will increase to incentivize more staking, and vice-versa.

The inflation mechanism based on the PD controller is now described in detail. First, there are some constant parameters needed as input:
- `r_max`: the maximum reward rate, expressed in annual percentage (or APR)
- `R_target`: the targeted staked token ratio (total staked supply / total token supply)
- `epochs_per_yr`: the expected number of epochs in a year
- `KP_nom`: the proportional gain factor (P in PD controller)
- `KD_nom`: the differential gain factor (D in PD controller)

Then there are some protocol values dependent on the epoch:
- `S`: the total supply of tokens
- `L`: the total number of staked tokens (locked in the PoS system)
- `I`: the most recent inflation amount (in units of tokens per epoch)
- `R_last`: the staked token ratio of the most recent epoch

The mechanism to calculate the new inflation amount `I_new` then follows:
1. Calculate some initial values that will be useful in the next steps
	- Max possible inflation `I_max = S * r_max / epochs_per_yr`
	- The gain factors for the new epoch
		- `KD = KD_nom * I_max`
		- `KP = KP_nom * I_max`
2. Calculate the error values for the PD controller
	- `EP = R_target - L/S`
	- `ED = EP - EP_last = R_last - L/S`
3. Calculate the control value for the PD controller
	- `C = KP * EP - KD * ED`
4. Calculate the new inflation amount
	- `I_new = max(0, min( I + C , I_max ))`

**Rewards distribution**

Once the inflation amount `I_new` has been determined, the new tokens are minted to the PoS account address, where they remain until a validator or delegator withdraws after an unbonding. The [reward distribution](https://specs.namada.net/economics/proof-of-stake/reward-distribution.html) scheme employed in Namada, based on the [F1 Fee distribution](https://drops.dagstuhl.de/opus/volltexte/2020/11974/pdf/OASIcs-Tokenomics-2019-10.pdf), allows for staking rewards to autocompound and be effectively automatically rebonded. Thus, no user transaction is required to claim staking rewards in Namada. In order to accomplish these things, the protocol must track the rewards owed to validators and delegators over the lifetime of their bonds.

The procedure for tracking the rewards is described as follows. First, over the course of an epoch, the Namada protocol tracks the fraction of the block rewards owed to each validator in the `consensus` validator set (the only validators who can earn rewards). Once an epoch has concluded, these fractions can be used to determine the number of tokens from `I_new` that are owed to each validator and their respective delegators. For each validator, the protocol keeps in storage a list of values ordered by epoch, where each value indicates the fractional increase in the validator's bonded stake due to the rewards earned since epoch 0. We call these lists the *rewards products*. Particularly, each entry of a validator's *rewards products*, corresponding to an epoch $e$, looks like:

$$\prod_{e=0}^e \big(1 + \frac{r_V(e)}{s_V(e)} \big),$$

where $r_V(e)$ is the amount of inflation tokens earned from block rewards in epoch $e$, and $s_V(e)$ is the validator's stake in $e$. To compute one of these values at the end of epoch $e$, we only need the value from epoch $e-1$ in addition to the new reward and stake. Then, the rewards accrued over a range of epochs can be determined as the ratio of these values at the boundary epochs of the range in question.

Two sets of rewards products are actually kept in storage - one (the above) considering validator self-bonds, and one considering delegations. The delegations reward products store values of the form:

$$\prod_{e=0}^e\big(1 + (1 - c_V(e))\frac{r_V(e)}{s_V(e)} \big),$$

where $c_V(e)$ is a validator's commission rate for delegations in epoch $e$.

There are several nice advantages of using the rewards products:
- The storage of the rewards products allows the validators' voting powers to be updated immediately by considering the rewards products along with their bonded staked tokens.
- The number of tokens to be credited to a delegator (or validator for a self-bond) only needs to be calculated at the moment of withdrawal by considering the original amount in the bond and the rewards products over the course of the bond's lifetime.
- Lots of unneeded iteration in storage can be avoided.
- Delegators do not need to repeatedly withdraw rewards and then redelegate.

As mentioned earlier, only validators in the `consensus` validator set have the ability to earn block rewards. Different portions of the block rewards are reserved for different behaviors; rewards are earned for proposing blocks, for signing (voting on) blocks, and also simply for being a member of the `consensus` validator set. Furthermore, all fees collected from transactions included in the block are owed solely to the block proposer.

The distribution of block rewards given to the proposer, signers, and other `consensus` validators is dependent on the cumulative stake of all validator signatures included in the block by the proposer. The distribution is designed in such a way that the proposer is always incentivized to include as many validator signatures as possible in the block. This behavior is encouraged because light client efficiency increases with the number of signatures. Namada's current configuration rewards 1.00 - 1.33% of the block rewards solely to the proposer. The cumulative tokens rewarded to the set of signing validators is distributed to each according to their weighted stake of the total signing stake. Likewise, the tokens reserved solely for being a `consensus` validator are proportional to the validator's stake.  More details are described in [here](https://specs.namada.net/economics/proof-of-stake/reward-distribution.html#distribution-to-validators).

# Conclusion / TLDR

This article has described the various parts of Namada's proof-of-stake system and the improvements that it employs relative to other proof-of-stake blockchain systems. These improvements are summarized as:

- **Epoched staking:** applying PoS state changes only at epoch boundaries makes light clients more efficient and allows for a DKG protocol to properly run in order to encrypt transactions in the mempool for optimized privacy.
- **Cubic slashing:** accounting for correlated validator behavior (infractions committed nearby each other) and increasing the severity of slashing punishments provides better security.
- **Auto-compounding rewards:** the concept of *rewards products* increases the protocol efficiency for processing PoS state changes and helps maximize rewards and UX for users (delegators).