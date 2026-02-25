# Game-Theoretic Foundations of Trust in Cordelia

**Deep dive document. Entry point: [WHITEPAPER.md](../../WHITEPAPER.md) Section 3.5, 9.1.**
**Quantitative trust model: [Network Model](../architecture/network-model.md) Section 4.9.**
**Threat analysis: [Threat Model](../architecture/threat-model.md) Sections 4.8, 8.3.**

---

## 1. Problem Statement

Cordelia is a distributed memory system where entities share knowledge
through groups. The fundamental game-theoretic question is:

> Under what conditions is honest memory sharing the rational strategy
> for a self-interested entity?

If honest sharing is not rational, the system degrades: entities
withhold valuable knowledge (underproduction) or inject false knowledge
(poisoning). Either failure mode destroys the value proposition.

We need to show that the protocol design creates a **cooperative
equilibrium** where honest sharing is the dominant strategy in repeated
interactions -- not because entities are altruistic, but because the
incentive structure makes cooperation more profitable than defection.

---

## 2. Model

### 2.1 Players

An entity `E_i` is a rational agent with:
- A private memory store `M_i` (sovereign, encrypted)
- Membership in zero or more groups `G_1, G_2, ..., G_k`
- A utility function `U_i` that values accurate knowledge

Entities are modelled as **von Neumann-Morgenstern rational** [1]:
they have consistent preferences over lotteries (uncertain outcomes)
and maximise expected utility. This is the weakest rationality
assumption that still yields tractable analysis -- we do not require
perfect information, unlimited computation, or identical utility
functions.

### 2.2 Strategies

Each entity chooses a **sharing strategy** for each group interaction:

- **Cooperate (C)**: Share accurate, high-novelty memories
- **Defect-Withhold (D_w)**: Free-ride on others' sharing without contributing
- **Defect-Poison (D_p)**: Share inaccurate or misleading memories

These are not binary choices -- entities can mix strategies with
probabilities `(p_c, p_w, p_p)` where `p_c + p_w + p_p = 1`.

### 2.3 Payoffs

Define payoffs per interaction round for entity `E_i`:

| E_i's strategy | Others cooperate | Others defect |
|----------------|-----------------|---------------|
| Cooperate | `b - c` | `-c` |
| Withhold | `b` | `0` |
| Poison | `b + d` | `d` |

Where:
- `b` = benefit from receiving accurate knowledge from others
- `c` = cost of producing and sharing accurate knowledge
- `d` = short-term gain from injecting false knowledge (manipulation,
  misdirection, competitive advantage)

In a single round, `D_w` dominates `C` (free-riding is always better
than contributing if there are no consequences), and `D_p` dominates
`D_w` when `d > 0`. This is a standard social dilemma / public goods
game.

The single-round analysis says the system should collapse. It doesn't,
because Cordelia is a **repeated game with observable actions and
endogenous punishment**.

---

## 3. Trust as a Mechanism

### 3.1 The Bayesian Trust Model

Each entity `E_i` maintains a local trust estimate for every peer
`E_j` it has interacted with. Trust is modelled as a Beta distribution:

```
T_ij ~ Beta(alpha_ij, beta_ij)

alpha_ij = 1 + sum(correct_memories_from_j)
beta_ij  = 1 + w * sum(incorrect_memories_from_j)
```

Where `w` is the **violation weight** (default: 10). The asymmetry is
deliberate: building trust requires many correct shares; destroying it
requires few violations.

**Properties of this model**:

| State | Distribution | E[T_ij] |
|-------|-------------|---------|
| No history | Beta(1, 1) | 0.50 |
| 10 correct | Beta(11, 1) | 0.92 |
| 100 correct | Beta(101, 1) | 0.99 |
| 100 correct, 1 violation | Beta(101, 11) | 0.90 |
| 100 correct, 2 violations | Beta(101, 21) | 0.83 |
| 100 correct, 3 violations | Beta(101, 31) | 0.77 |

The Beta distribution is conjugate to the Bernoulli likelihood, meaning
each observation updates the posterior analytically (no approximation
needed). This is computationally trivial and mathematically exact.

### 3.2 Trust as a Gating Function

Trust modulates how an entity processes incoming memories:

```
acceptance_probability(memory_from_j) = f(E[T_ij], memory_novelty)
```

Where `f` is monotonically increasing in both arguments. High-trust
peers' memories are accepted readily. Low-trust peers' memories are
scrutinised or rejected. The exact form of `f` is a local policy
decision -- entity sovereignty means each entity chooses its own
acceptance function.

The key insight: **trust gates access to the benefit `b`**. An entity
that defects (poison or withhold) eventually loses trust, which means
its future memories are rejected, which means it loses access to the
group's knowledge. This transforms the single-round payoff matrix.

### 3.3 Modified Payoff with Trust

In a repeated game with trust, entity `E_i`'s long-run expected utility
from strategy `sigma_i` in group `G` is:

```
U_i(sigma_i) = sum_{t=0}^{inf} delta^t * [
    b * P(receive | T_{ji}(t)) - c * I(share_i(t)) + d * I(poison_i(t))
]
```

Where:
- `delta` is the discount factor (how much the entity values future
  payoffs relative to present)
- `P(receive | T_{ji}(t))` is the probability of receiving valuable
  memories, which depends on how much others trust `E_i` at time `t`
- `I(share_i(t))` is 1 if `E_i` shares honestly at time `t`
- `I(poison_i(t))` is 1 if `E_i` poisons at time `t`

The crucial term is `P(receive | T_{ji}(t))`. After a poisoning event:

```
T_{ji} drops by ~10 units of beta (violation weight)
P(receive) drops proportionally
Future b is reduced for many rounds
```

---

## 4. Equilibrium Analysis

### 4.1 Folk Theorem Application

The repeated game with trust-based punishment falls under the **Folk
Theorem** [2]: in an infinitely repeated game with sufficiently patient
players (high `delta`), cooperation can be sustained as a Nash
equilibrium using trigger strategies.

Cordelia's trust mechanism implements a **graduated trigger strategy**:
- Cooperation is maintained as long as trust remains high
- A single violation triggers partial punishment (trust drop)
- Multiple violations trigger escalating punishment
- Recovery is possible but expensive (requires many correct shares)

This is more forgiving than a grim trigger (permanent punishment) and
more robust than tit-for-tat (which is vulnerable to noise). The Beta
distribution provides a natural graduated response.

### 4.2 Cooperation as Dominant Strategy

**Theorem (informal)**: For entities with discount factor `delta > c/b`
(i.e., entities that value future group access more than the cost of
contributing), full cooperation is the unique stable Nash equilibrium.

**Proof sketch**:

1. **Withholding is detectable**. In a chatty or moderate group, the
   absence of contributions from an entity is observable over time. The
   group can reduce its trust in non-contributors (though this is
   weaker than poisoning detection).

2. **Poisoning is detectable**. When a memory is later found to be
   inaccurate, the author_id (immutable provenance) identifies the
   source. Trust drops by `w` per violation.

3. **Cost of defection exceeds benefit**. A single poisoning episode
   yields gain `d` but costs `w * b * delta / (1 - delta)` in reduced
   future access (the discounted sum of lost benefits from reduced
   trust). For `delta > c/b` and `w >= 10`:

```
   d < w * b * delta / (1 - delta)
```

   This holds for any reasonable `d` when `w = 10` and `delta > 0.5`
   (entity values group membership for more than 2 rounds).

4. **No profitable deviation exists**. Any mixed strategy that includes
   poisoning or withholding yields lower expected long-run utility than
   pure cooperation, because the trust penalty on future payoffs exceeds
   the one-time gain from defection.

### 4.3 Pareto Optimality

The full-cooperation equilibrium is also **Pareto optimal**: no entity
can improve its utility without reducing another entity's utility. This
is because cooperation maximises the total knowledge in the group,
which benefits all members. Any deviation (withholding or poisoning)
reduces total knowledge and therefore reduces at least one entity's
utility.

This is the strongest possible efficiency result: not only is
cooperation a Nash equilibrium (no one wants to deviate), it is also
socially optimal (no better outcome exists for the group as a whole).

---

## 5. Adversarial Analysis

### 5.1 Trust Building Attack

**Attack**: Entity shares 100 correct memories to build trust
(E[T] = 0.99), then shares 1 poisoned memory.

**Cost**: Weeks to months of producing genuine, high-novelty content
(novelty filtering prevents recycling low-value content).

**Gain**: 1 poisoned memory accepted by high-trust peers.

**Detection aftermath** (quantified from [Network Model](../architecture/network-model.md)):

```
Pre-attack:    E[T] = 0.99  (Beta(101, 1))
After 1 violation: E[T] = 0.90  (Beta(101, 11))  -- 10% drop
After 2:       E[T] = 0.83  (Beta(101, 21))  -- 17% drop
After 3:       E[T] = 0.77  (Beta(101, 31))  -- 23% drop
```

**Recovery cost**: Each violation requires ~100 correct shares to
recover to the pre-attack trust level. 3 violations require ~300
correct shares -- months of good behaviour.

**Result**: The attack succeeds once but is self-limiting. The attacker
must invest weeks per poisoned memory and risks losing all accumulated
trust. The cost-benefit ratio is unfavourable for any sustained
campaign.

### 5.2 Slow Poisoning

**Attack**: Share memories with small, hard-to-detect inaccuracies.
Each individual memory is plausible but subtly wrong.

**Defence layers**:

1. **Novelty engine**: Inaccuracies that contradict existing high-trust
   memories score low on novelty (they conflict with established
   knowledge). The novelty threshold provides a first filter.

2. **Cross-validation**: When multiple entities in a group share
   memories about the same topic, inconsistencies surface. An entity
   whose memories consistently diverge from the group consensus loses
   trust, even if no individual memory is provably wrong.

3. **Time-delayed verification**: Some inaccuracies are only detectable
   when the knowledge is applied. If an entity acts on a poisoned
   memory and the outcome is bad, the memory is flagged and the
   author's trust drops. This is slow but reliable.

4. **Self-distrust**: The entity that received the poisoned memory can
   quarantine its own uncertain memories, preventing further
   propagation. This limits blast radius.

**Residual risk**: Slow poisoning is the most sophisticated attack and
the hardest to detect. The defence is probabilistic, not deterministic.
However, the attacker must produce plausible content that passes novelty
filtering -- this requires genuine domain knowledge, which is itself
costly. See Section 7 (Proof-of-Useful-Work) for the information-
theoretic argument.

### 5.3 Collusion

**Attack**: Multiple entities coordinate to corroborate each other's
false memories, making them appear independently verified.

**Defence**: Trust is **local** -- each entity computes its own trust
independently. There is no global reputation to game. Colluding
entities can vouch for each other, but:

1. An honest entity's trust in a colluder is based on that entity's
   *own* experience, not on the colluder's trust score with others.
2. If colluders share memories that are later found inaccurate, all
   colluders' trust drops independently.
3. Copy-on-write provenance means the original author is always
   identifiable -- re-sharing doesn't launder authorship.

**Formal bound**: For a collusion group of size `k` targeting an
honest entity with `n` trusted peers, the colluders must represent a
fraction `k / (k + n)` of the target's trusted peers to dominate the
knowledge stream. With the governor's churn rotation (20% hourly), an
honest entity continuously discovers new non-colluding peers, diluting
the colluders' influence.

### 5.4 Sybil + Trust Gaming

**Attack**: Create many fake identities, build trust on each, then
coordinate poisoning across all.

**Cost**: Each Sybil must independently build trust (there is no
reputation transfer). At 100 correct shares per Sybil (weeks), the
cost scales linearly with the number of Sybils. For 10 coordinated
Sybils: months of producing genuine content across all identities.

**Defence**: Connection limits per subnet (R2), reputation gating (R3),
and invite graphs (R4) raise the cost of getting Sybils into a target's
peer table in the first place. Combined with the trust-building cost,
this makes large-scale Sybil+trust attacks prohibitively expensive for
any attacker without nation-state resources.

---

## 6. Natural Selection as Evolutionary Game Theory

### 6.1 Memory Fitness

Cordelia's TTL and access-counting mechanism creates an evolutionary
dynamic. Define the **fitness** of a memory `m` in group `G` as:

```
fitness(m) = access_count(m) / age(m)
```

Memories with high fitness (frequently accessed relative to age) survive
TTL expiry. Memories with low fitness (rarely accessed) expire.

This is replicator dynamics applied to information: the population of
memories in a group evolves over time, with access frequency as the
selection pressure. High-utility memories reproduce (are shared,
copied, referenced) while low-utility memories die.

### 6.2 Cultural Selection Pressure

Group culture (chatty/moderate/taciturn) determines the selection
pressure:

- **Chatty** groups have weak selection: everything is pushed, memories
  expire only via TTL. This suits small, high-trust teams where volume
  is manageable.
- **Taciturn** groups have strong selection: memories must be actively
  sought. Only memories that entities specifically search for survive.
  This suits large public groups where noise filtering is critical.
- **Moderate** groups balance: headers are pushed (making memories
  discoverable) but full content is demand-fetched. Memories that are
  discovered but never fetched expire.

This maps to biological environments: resource-rich environments
(chatty) support more species with less selection pressure; resource-
scarce environments (taciturn) drive intense competition and rapid
adaptation.

### 6.3 Governance as Weighted Voting

Protocol upgrades and group policy changes use access-weighted voting.
Memories with higher access counts carry more weight. This is analogous
to **proof-of-stake** but with a crucial difference: the "stake" is not
purchased -- it is earned through demonstrated utility. You cannot buy
governance weight; you must produce knowledge that others find valuable.

This creates a positive feedback loop: entities whose memories are most
valued have the most governance influence, and they use that influence
to maintain the conditions that produce valuable memories (accurate
sharing, high novelty standards, appropriate culture settings).

---

## 7. Proof-of-Useful-Work (Conjecture)

This section describes an open conjecture. Formal proof is future work
(R4+). We include it because it underpins the long-term economic model.

### 7.1 The Claim

> The cost of producing content that passes Cordelia's novelty filtering
> and trust calibration is bounded below by the information-theoretic
> entropy of the content relative to the receiver's existing knowledge.

In simpler terms: you cannot cheaply fake useful knowledge. Producing
a memory that passes the novelty engine (it must be surprising relative
to what the receiver already knows) and survives trust calibration (it
must be accurate when tested against reality) requires genuine cognitive
work proportional to the value of the knowledge.

### 7.2 The Argument (Informal)

Shannon's information theory [3] provides the foundation. The novelty
score of a memory is (approximately) its information-theoretic surprise:

```
novelty(m | context C) ~ -log P(m | C)
```

A memory that is predictable given context `C` has low novelty and is
filtered out. A memory that is surprising has high novelty and passes.

For an attacker to produce high-novelty false content:

1. The content must be surprising (high entropy relative to receiver's
   context) -- otherwise the novelty engine rejects it.
2. The content must be plausible (low cross-entropy with the receiver's
   world model) -- otherwise it is immediately flagged as suspicious.
3. The content must survive verification (accurate when tested) --
   otherwise trust drops.

Requirements 1 and 2 are in tension: surprising but plausible content
is exactly the definition of valuable knowledge. Requirement 3 rules
out all fabrication. The intersection of these three requirements is
genuine knowledge -- and producing genuine knowledge is the "work" in
Proof-of-Useful-Work.

### 7.3 Comparison to Other Proof Systems

| System | Work | Energy | Sybil resistance | Value of work |
|--------|------|--------|-----------------|---------------|
| Bitcoin PoW | Hash collision | Electricity, ASICs | Strong (thermodynamic cost) | Zero (discarded after use) |
| Ethereum PoS | Capital lockup | Opportunity cost | Strong (economic cost) | Capital allocation signal |
| Cardano Ouroboros | Stake selection | Minimal compute | Strong (stake grinding resistant) | Capital allocation signal |
| Cordelia PoUW | Knowledge production | Cognitive effort | *Conjectured* (entropy cost) | The work IS the value |

The key difference: in PoW, work and value are separate (the hash
computation produces nothing useful). In Cordelia, the work of
producing high-novelty accurate memories IS the value the system
provides. There is no wasted energy.

### 7.4 Open Questions

The following require formal analysis before the PoUW claim can be
made with confidence:

1. **Formal entropy bound**: Can we prove a Shannon lower bound on the
   cost of producing content that passes novelty filtering in context C?

2. **LLM adversary**: Can a language model produce high-entropy
   plausible-but-false memories that survive trust calibration? If so,
   the entropy argument breaks down for AI-equipped attackers.

3. **Content recycling**: Can an attacker repackage known content to
   pass novelty filtering? The novelty engine scores against the
   receiver's existing knowledge, but an attacker might target receivers
   whose context they don't have.

4. **Collusion amplification**: Do coordinated Sybils reduce the
   per-entity cost of producing novel content? (Likely no -- novelty
   is relative to the receiver, not the sender.)

5. **Comparison theorems**: Can we formally relate PoUW security
   guarantees to PoW/PoS security guarantees? What is the equivalent
   of "51% attack" in a knowledge-based proof system?

---

## 8. Self-Distrust and Metacognition

A unique feature of Cordelia's trust model is that it applies reflexively.
An entity can distrust its own memories.

### 8.1 Mechanism

```
entity.flag_low_confidence(memory_id, reason)
  -> memory.confidence = "low"
  -> memory excluded from relay propagation
  -> memory excluded from group sharing
  -> memory retained locally (entity sovereignty: even quarantined
     memories belong to the entity)
```

### 8.2 Game-Theoretic Significance

Self-distrust serves three functions:

1. **Blast radius limitation**: If an entity suspects it has been
   poisoned (e.g., received inaccurate memories from a now-distrusted
   peer), it can quarantine derived memories before they propagate.

2. **Honest signalling**: An entity that self-distrusts signals to the
   trust mechanism that it applies quality standards to its own output.
   This is a costly signal (it reduces the entity's sharing volume) that
   correlates with quality.

3. **Metacognitive capability**: In the context of AI agents, self-
   distrust is a mechanism for calibrating confidence. An agent that
   "knows what it doesn't know" produces higher-quality memory output
   than one that shares everything with equal confidence.

This is Dennett's "competence without comprehension" [4] applied at
the system level: the trust mechanism produces collectively intelligent
behaviour without requiring any individual entity to understand the
game theory. Each entity simply maximises its own utility using local
trust signals, and the emergent result is a cooperative equilibrium
with quality control.

---

## 9. Entity Sovereignty as Game-Theoretic Constraint

### 9.1 The Invariant

> Entity trust has primacy over all group policies. Always.

This is not just a security property -- it is a game-theoretic design
choice. By making entity sovereignty inviolable, the protocol ensures
that no mechanism (group policy, governance vote, administrative action)
can force an entity to accept memories it doesn't want.

### 9.2 Why This Matters for Equilibrium

Without sovereignty, a majority coalition in a group could force-feed
memories to minority members. This creates a new attack vector:
**governance capture** followed by policy injection. The sovereign entity
cannot be attacked through governance -- only through the trust channel,
which requires producing actually-useful memories.

Sovereignty also prevents **lock-in**: an entity that disagrees with a
group's direction can leave with its memories intact. The exit option
disciplines group governance (the threat of departure keeps groups
honest), which is exactly Hirschman's exit/voice/loyalty framework [5]
applied to knowledge networks.

### 9.3 Sovereignty and the Cooperative Equilibrium

Sovereignty strengthens the cooperative equilibrium because it removes
coercive strategies from the game. When entities cannot be forced to
participate, all participation is voluntary. Voluntary participation
in a repeated game with observable actions converges on cooperation
(by the Folk Theorem) more reliably than coerced participation, because
there is no resentment dynamic and no incentive to sabotage from within.

---

## 10. Frame Memory and Cooperative Amplification

### 10.1 Observation

An empirical observation from the Cordelia development process: when
an entity's L1 hot context includes frame memory (key intellectual
references, conceptual vocabulary, reasoning style), the productivity
of human-agent collaboration increases non-linearly. The agent does not
merely recall facts -- it reasons within the correct conceptual
framework from the first exchange.

### 10.2 Mechanism

Frame memory (e.g., `key_refs: [shannon, denning, von_neumann_morgenstern,
minsky, dennett]`) shifts the agent's prior distribution over reasoning
paths. Without frame memory, the agent starts from a generic prior --
broad, shallow, equidistant from all design spaces. With frame memory,
the prior is concentrated in the region where the referenced frameworks
intersect.

Formally, frame memory reduces the **Kullback-Leibler divergence**
between the agent's default prior `P` and the task-optimal distribution
`Q`:

```
D_KL(Q || P_with_frame) << D_KL(Q || P_default)
```

This means fewer conversational turns are needed to reach productive
design work. Each turn that would have been spent navigating to the
right design space is instead spent doing design work within it.

### 10.3 Game-Theoretic Implication

Frame memory creates a **cooperative amplification** effect that
strengthens the cooperative equilibrium described in Section 4.

In the standard model, the benefit `b` of receiving accurate memories
from peers is constant. With frame memory, `b` becomes a function of
the receiver's frame:

```
b(m, frame) = b_data(m) + b_frame(m, frame)
```

Where `b_frame` is the additional value extracted from memory `m` when
processed through the correct conceptual frame. A memory about "natural
selection for cached data" yields `b_data` to any receiver, but
additionally yields `b_frame` to a receiver whose frame includes
Denning's working set model and Shannon's information theory.

This means entities with better frame memory extract more value from
the same group knowledge. Frame memory is a **multiplier** on the
cooperative surplus. Groups where members share frame memory (common
conceptual vocabulary) extract more total value from cooperation than
groups where members have disjoint frames.

This creates a second-order incentive: entities are incentivised not
just to share data memories, but to share frame memories (conceptual
references, reasoning patterns, metaphors) that increase the group's
capacity to extract value from future knowledge sharing.

### 10.4 The Martin Corollary

Observed: a team member struggled with detailed system specifications
until shown an architecture diagram that established the conceptual
frame. Once the frame was loaded, the same detail that was previously
noise became comprehensible. Frame memory is a **prerequisite** for
data memory to be useful.

This has a practical design implication: the system should ensure that
frame memory is loaded before data memory. L1 structure should place
key_refs, style, and conceptual anchors before active state, blockers,
and task detail. The agent (or human) needs the coordinate system
before the coordinates.

---

## 11. Summary

The game-theoretic structure of Cordelia creates cooperation through
six reinforcing mechanisms:

1. **Bayesian trust with asymmetric decay** makes defection costly
   (Section 3). One violation costs 10x the investment of one correct
   share.

2. **Repeated game dynamics** make cooperation the dominant long-run
   strategy for any entity that values group membership (Section 4).

3. **Local trust computation** prevents reputation attacks, Sybil
   gaming, and governance capture (Section 5).

4. **Natural selection via TTL + access counting** ensures memory
   quality increases over time (Section 6).

5. **Entity sovereignty** removes coercive strategies and strengthens
   the cooperative equilibrium through voluntary participation
   (Section 9).

6. **Frame memory amplification** creates a multiplier on cooperative
   surplus, incentivising entities to share not just data but conceptual
   frameworks that increase the group's capacity to extract value
   (Section 10).

The system does not require altruism. It requires only that entities
are rational (prefer more utility to less) and patient (value future
group access). Under these conditions, the protocol structure makes
cooperation not just possible but inevitable.

The frame memory insight adds a dimension not present in standard
public goods games: the value of cooperation is not fixed but increases
with shared conceptual infrastructure. Groups that invest in common
frames extract superlinear returns from knowledge sharing. Memory, in
this model, is not just accumulated data -- it is the cognitive
coordinate system that determines what data means.

---

## References

[1] J. von Neumann and O. Morgenstern, *Theory of Games and Economic
Behavior*, Princeton University Press, 1944.

[2] D. Fudenberg and J. Tirole, *Game Theory*, MIT Press, 1991.
Folk Theorem and repeated game equilibria.

[3] C. E. Shannon, "A Mathematical Theory of Communication," *Bell
System Technical Journal*, vol. 27, no. 3, 1948.

[4] D. C. Dennett, *From Bacteria to Bach and Back*, W. W. Norton,
2017.

[5] A. O. Hirschman, *Exit, Voice, and Loyalty*, Harvard University
Press, 1970.

---

*Version 1.0 -- 2026-01-31*
*Companion to [WHITEPAPER.md](../../WHITEPAPER.md). Quantitative model from [Network Model](../architecture/network-model.md).*
