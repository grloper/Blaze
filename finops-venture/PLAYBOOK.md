# The FinOps Micro-Agency Playbook
### Operational blueprint for a 4-person cloud cost recovery practice

> **Read this first.** Every legal template in this document is a *drafting starting point*, not legal advice. Before you sign anything or touch a client account, have a licensed attorney in your jurisdiction review your MSA, liability language, and entity structure. A law student on the team is an asset for drafting and negotiation — not a substitute for a licensed reviewer. Likewise: form a limited-liability entity and buy professional liability (E&O) insurance *before* client #1. Contract clauses can be challenged. Insurance pays.

---

## 0. The business in one paragraph

Funded startups overspend on AWS by 20–35%, not through incompetence but through **unowned surface area** — resources nobody deleted, environments nobody scheduled, commitments nobody bought. The engineers know how to fix it and never will, because it competes with shipping and because deleting things in production is career-risk with no career-reward. We sell the sweep and, critically, **we absorb the risk-ownership** — read-only access, findings with dollar amounts and risk ratings, changes delivered as reviewable pull requests the client merges themselves. First engagement is a fixed-fee audit. The real business is the retainer that stops it from growing back.

**Unit economics target:**

| Milestone | Clients | Revenue | Your hours/mo |
|---|---|---|---|
| Month 1 | 1 audit | $1,000 | ~30 |
| Month 3 | 3 audits + 1 retainer | $6,500 | ~60 |
| Month 6 | 4 audits + 5 retainers | $18,000 | ~90 |
| Month 12 | 4 audits + 10 retainers | $32,000 | ~120 (needs a 2nd deliverer) |

---

## 1. Lead Generation & Prospecting

### 1.1 Ideal Customer Profile

**Target:**
- Raised **Seed or Series A, 6–24 months ago**. Sweet spot is **9–18 months**.
- **10–50 employees**, of which **5–25 are engineers**
- **AWS-primary.** Ignore GCP/Azure for the first 6 months — one playbook, executed well.
- B2B SaaS, data/AI, fintech, devtools. Anything with real infrastructure.
- Monthly AWS spend **$8k–80k**. Below $8k the fee doesn't justify itself; above $80k you'll hit a procurement process you can't yet navigate.

**Why the 9–18 month window specifically:** Under 6 months, the money is fresh and nobody cares about efficiency — they're hiring, not trimming. Past 24 months they've either raised again or they're distressed and have no budget. In the 9–18 month window the board has started asking about burn multiple and runway, and the CTO has just been handed "reduce spend" with no time to do it. That is the exact moment your email is not spam.

**Anti-profile — do not waste cycles:**
- Pre-seed / bootstrapped (bill too small, no budget)
- 100+ employees (platform team exists, procurement exists, 4-month sales cycle)
- Series B+ with a named FinOps or Platform Cost owner
- Heroku/Vercel/Render-only shops (no lever to pull)
- Agencies and consultancies (they'll try to subcontract you at cost)

### 1.2 Where to find them

**Free sources, in order of yield:**

1. **Y Combinator company directory** (`ycombinator.com/companies`) — filter by batch. Batches from 12–24 months ago are exactly your window. Shows team size and hiring status. Free, structured, high density.
2. **Wellfound / LinkedIn job posts.** A 20-person startup posting for a "DevOps Engineer" or "Platform Engineer" is announcing publicly that they have infrastructure pain and nobody to own it. This is the single highest-signal source in the entire list.
3. **Crunchbase free tier** — filter: funding round = Seed/Series A, announced 6–24 months ago, industry = software.
4. **TechCrunch / Axios Pro Rata / EU: Sifted** funding announcements. Daily, free.
5. **AWS Activate portfolio companies** and accelerator cohort pages.

**Paid, worth it from month 2:**
- **LinkedIn Sales Navigator** (~$100/mo, one seat, free trial first). Filters: headcount 11–50, industry Software Development, "Recent funding events." Titles: CTO, VP Engineering, Head of Platform, Head of Infrastructure, Founding Engineer. At sub-30 headcount also target CEO/Founder — they still own the budget call.
- **Apollo.io / Hunter.io** for email discovery (both have free tiers). Most startups are `first@company.com`. Verify before sending; deliverability is your whole channel.

**Signal-based prospecting — converts ~10× cold:**
- Someone posts publicly complaining about an AWS bill → reply with something genuinely useful, *then* DM. Never DM first.
- A company announces layoffs or "a focus on efficiency" → burn is now a board-level agenda item.
- An engineering blog post about scaling → they have volume, and volume means waste.

### 1.3 The list

One spreadsheet. These columns, no more:

`Company | Round | Funding date | Headcount | Est. engineers | Contact | Title | LinkedIn | Email | Hook/signal | Channel | Status | Last touch | Next action`

**Build 100 rows in one sitting.** It takes 3–4 hours. Do not start sending until the list exists — mixing research and outreach halves the throughput of both.

### 1.4 Honest volume math

| Channel | Reply rate | Positive | Calls | Closes |
|---|---|---|---|---|
| Cold email (100 sent) | 5–10% | 2–4% | 1–2 | ~1 |
| LinkedIn DM (100 sent) | 15–25% | 5–8% | 3–5 | 1–2 |
| Warm intro (10 asked) | 60%+ | — | 5–6 | 1–2 |

**Read that table again.** Ten warm intros outperform a hundred cold emails. Week one is warm network — every person the four of you collectively know who works anywhere with cloud infrastructure. Cold outreach runs in parallel because it takes longer to mature, not because it's better.

To land a client inside 14 days you need roughly **150–200 total touches**. That's 15/day across the team. It is boring. It is also the entire job.

---

### 1.5 Cold Email — Template A → CTO / VP Engineering

*Technical, peer-to-peer, proves you're an engineer within the first two lines.*

**Subject lines (rotate, test):**
- `your NAT gateway bill`
- `EBS snapshots at [Company]`
- `quick one — AWS`

```
Hi [First],

I'm a DevOps engineer. I do one narrow thing: find waste in AWS accounts at
Series A startups and fix it.

Three things I find in almost every account at your stage:

- Orphaned EBS volumes and snapshots with no lifecycle policy — usually
  $500–3k/mo of pure nothing
- Dev and staging running 168 hrs/week when they're used for ~50
- Traffic to S3 and ECR routed through NAT Gateway instead of VPC endpoints.
  This one is frequently the biggest single line item and almost nobody
  looks at it

Give me a read-only IAM role and I'll send a findings doc in 48 hours with
dollar amounts and a risk rating per item. If the annualized number is under
$2k, you owe nothing and we're done.

Worth 20 minutes?

[Name]
[GitHub] · [LinkedIn]
```

**Why it works:** Specific enough that no SDR could have written it. Names three problems they will personally recognize. The ask is a read-only role, not a meeting. Risk is explicitly zero. Under 150 words. No attachment, no deck, no calendar link in the first email.

---

### 1.6 Cold Email — Template B → CEO / CFO

*Runway language. Zero technical vocabulary.*

**Subject:** `3 months of runway hiding in your AWS bill`

```
Hi [First],

Congrats on the [Series A].

Most companies your size are overspending on AWS by 20–35% — not through
anything careless, just resources nobody has had time to clean up. On a
$20k/month bill that's $50–80k a year. At your burn rate, that's runway.

I run a fixed-scope audit: read-only access, 48 hours, and a document showing
exactly where the money goes and what it takes to get it back. If I find less
than $2,000/year, there's no charge.

No commitment beyond the 48 hours. Want me to take a look?

[Name]
```

**Why it works:** Translates the entire product into the one metric a CEO thinks in daily. The percentage is checkable, so it reads as informed rather than salesy. "No charge" removes the decision entirely — there is nothing to say no *to*.

**Routing rule:** CEO under ~30 headcount, CTO above. Above 30, a CEO email gets forwarded to the CTO with no context and dies.

---

### 1.7 LinkedIn DM

**Connection request note** (hard limit 300 characters):

```
Hi [First] — I do AWS cost audits for Series A startups, usually finding the
20–30% that's orphaned resources and idle non-prod. Saw [Company] is hiring a
platform engineer. Mind if I send the 3 things I'd check first?
```

**Follow-up once accepted:**

```
Thanks for connecting.

Genuinely, the three things I'd look at first at [Company]'s stage:

1. Unattached EBS volumes and snapshots with no lifecycle policy
2. Non-prod environments running nights and weekends
3. VPC endpoints for S3 and ECR — NAT Gateway data processing is usually the
   most surprising line on the bill

You can check all three yourself in about 20 minutes, and I'm happy to walk
you through it either way.

If it's easier, give me a read-only IAM role and I'll send the full findings
doc in 48 hours with dollar amounts attached. No charge if it comes in under
$2k/yr annualized.
```

**The counterintuitive part:** *"You can check all three yourself"* is the highest-converting sentence in this document. It proves you are not farming a lead, and it costs you nothing — because they won't. And if they do, they'll find something, and then you're the person who was right.

---

## 2. Overcoming Access & Security Friction

This is where deals die. Solve it structurally rather than with reassurance.

### 2.1 Never ask for access keys

Ask them to create a **cross-account IAM role** that trusts your AWS account, gated by an `ExternalId`. This is [the AWS-documented pattern for third-party access](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_common-scenarios_third-party.html), and proposing it correctly signals competence before you've said a word about cost.

Send them a CloudFormation template so it is one click and zero thinking.

### 2.2 The minimal read-only policy

Do **not** ask for the AWS-managed `ReadOnlyAccess` policy. It is wildly over-permissive for this job — it grants object listing, parameter reads, and (via `lambda:GetFunction`) a presigned URL to download their source code. Asking for it is a competence signal in the wrong direction, and a sharp CTO will reject it on sight.

The policy below is scoped to exactly what a cost audit needs. **It cannot read a single byte of customer data or application code.**

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "BillingAndCostAnalysis",
      "Effect": "Allow",
      "Action": [
        "ce:Get*",
        "ce:List*",
        "ce:Describe*",
        "cost-optimization-hub:ListEnrollmentStatuses",
        "cost-optimization-hub:GetPreferences",
        "cost-optimization-hub:GetRecommendation",
        "cost-optimization-hub:ListRecommendations",
        "cost-optimization-hub:ListRecommendationSummaries",
        "cost-optimization-hub:ListEfficiencyMetrics",
        "compute-optimizer:Get*",
        "compute-optimizer:Describe*",
        "budgets:Describe*",
        "budgets:ViewBudget",
        "savingsplans:Describe*",
        "savingsplans:List*",
        "pricing:Get*",
        "pricing:Describe*",
        "cur:Describe*",
        "freetier:GetFreeTierUsage"
      ],
      "Resource": "*"
    },
    {
      "Sid": "ResourceInventoryMetadataOnly",
      "Effect": "Allow",
      "Action": [
        "ec2:Describe*",
        "autoscaling:Describe*",
        "elasticloadbalancing:Describe*",
        "rds:Describe*",
        "rds:ListTagsForResource",
        "elasticache:Describe*",
        "es:Describe*",
        "es:List*",
        "dynamodb:Describe*",
        "dynamodb:List*",
        "ecs:Describe*",
        "ecs:List*",
        "eks:Describe*",
        "eks:List*",
        "lambda:List*",
        "lambda:GetFunctionConfiguration",
        "lambda:GetAccountSettings",
        "redshift:Describe*",
        "kinesis:Describe*",
        "kinesis:List*",
        "sqs:ListQueues",
        "sns:ListTopics",
        "cloudfront:List*",
        "cloudfront:Get*",
        "route53:List*",
        "apigateway:GET",
        "backup:List*",
        "backup:Describe*",
        "fsx:Describe*",
        "efs:Describe*",
        "sagemaker:List*",
        "sagemaker:Describe*"
      ],
      "Resource": "*"
    },
    {
      "Sid": "StorageConfigurationNotContents",
      "Effect": "Allow",
      "Action": [
        "s3:ListAllMyBuckets",
        "s3:GetBucketLocation",
        "s3:GetBucketTagging",
        "s3:GetLifecycleConfiguration",
        "s3:GetBucketVersioning",
        "s3:GetIntelligentTieringConfiguration",
        "s3:GetMetricsConfiguration",
        "s3:GetAnalyticsConfiguration",
        "s3:GetStorageLensConfiguration",
        "s3:ListStorageLensConfigurations"
      ],
      "Resource": "*"
    },
    {
      "Sid": "UtilizationMetrics",
      "Effect": "Allow",
      "Action": [
        "cloudwatch:GetMetricData",
        "cloudwatch:GetMetricStatistics",
        "cloudwatch:ListMetrics",
        "cloudwatch:DescribeAlarms",
        "logs:DescribeLogGroups",
        "logs:DescribeSubscriptionFilters"
      ],
      "Resource": "*"
    },
    {
      "Sid": "AccountStructureAndAdvisor",
      "Effect": "Allow",
      "Action": [
        "organizations:Describe*",
        "organizations:List*",
        "support:DescribeTrustedAdvisorChecks",
        "support:DescribeTrustedAdvisorCheckResult",
        "support:DescribeTrustedAdvisorCheckSummaries",
        "tag:GetResources",
        "tag:GetTagKeys",
        "tag:GetTagValues"
      ],
      "Resource": "*"
    }
  ]
}
```

**The four exclusions to say out loud in the pitch** — these are the sale:

| Excluded | Why it matters to them |
|---|---|
| `s3:GetObject`, `s3:ListBucket` | We cannot read or even enumerate a single object in your buckets |
| `lambda:GetFunction` | Returns a download URL for the deployment package. We take `GetFunctionConfiguration` only — memory and timeout, never code |
| `logs:GetLogEvents`, `FilterLogEvents` | We see log group *sizes* and retention settings. Never a log line |
| `ssm:GetParameter*`, `secretsmanager:*`, `kms:Decrypt` | Not present at all. No secrets, no credentials, no keys |

**Trust accelerators to state in the same breath:**
- `ExternalId` required on the role trust policy (prevents the confused-deputy problem)
- They revoke instantly by deleting one role — no offboarding process, no ticket
- **Every API call we make lands in their CloudTrail.** They can audit exactly what we looked at, in real time, without asking us
- MFA enforced on our side, and we'll name the specific principal ARN that will assume the role

### 2.3 Data Safety Statement — drop into every pitch

> **Data safety.** We access your AWS account through a cross-account IAM role that you create, control, and can revoke at any time by deleting it — we never receive or store long-lived credentials. The role is strictly read-only and is scoped to billing, resource configuration, and utilization metrics. It explicitly excludes permission to read the contents of your S3 objects, your application source code, your logs, your database records, and your secrets or parameter store. Every API call we make is recorded in your own CloudTrail, so you can independently audit exactly what we accessed at any point, during or after the engagement. We do not export, copy, or retain your data beyond the aggregate cost and configuration figures required to produce your findings report; all working material is deleted within 30 days of engagement close on request. We will sign your NDA, or provide ours, before access is granted.

That paragraph closes the security objection roughly 80% of the time when it appears *before* they ask.

---

## 3. Technical Fast-Audit Cheat Sheet

The first 30 minutes. This sequence pulls ~80% of the findings before you've thought about anything.

**Setup:**
```bash
export AWS_PROFILE=client-readonly   # assumed cross-account role
export S=2026-04-01 E=2026-07-01     # 3-month window
aws sts get-caller-identity           # confirm you're where you think you are
```

### The 12 commands

**1 — Where the money actually goes (always start here)**
```bash
aws ce get-cost-and-usage \
  --time-period Start=$S,End=$E --granularity MONTHLY \
  --metrics UnblendedCost --group-by Type=DIMENSION,Key=SERVICE \
  --output table
```

**2 — AWS's own recommendations, pre-aggregated with dollar values**
```bash
aws cost-optimization-hub list-recommendation-summaries --group-by ActionType
aws cost-optimization-hub list-recommendations --max-results 100 \
  --query 'items[].[currentResourceType,actionType,estimatedMonthlySavings]' --output table
```
> If it returns a not-enrolled error, **that is finding #1** — Cost Optimization Hub is free and they never turned it on.

**3 — Rightsizing across every compute type**
```bash
aws compute-optimizer get-ec2-instance-recommendations \
  --query 'instanceRecommendations[?finding==`OVER_PROVISIONED`].[currentInstanceType,recommendationOptions[0].instanceType,recommendationOptions[0].estimatedMonthlySavings.value]' \
  --output table
aws compute-optimizer get-auto-scaling-group-recommendations
aws compute-optimizer get-ebs-volume-recommendations
aws compute-optimizer get-lambda-function-recommendations
aws compute-optimizer get-rds-database-recommendations
```

**4 — Unattached EBS volumes (free money, zero risk)**
```bash
aws ec2 describe-volumes --filters Name=status,Values=available \
  --query 'Volumes[].[VolumeId,Size,VolumeType,CreateTime]' --output table
```
> Cost: gp3 ≈ $0.08/GB-mo, gp2 ≈ $0.10/GB-mo. Sum sizes × rate = the number for the report.

**5 — Orphaned snapshots**
```bash
aws ec2 describe-snapshots --owner-ids self \
  --query 'Snapshots[?StartTime<=`2025-10-01`].[SnapshotId,VolumeSize,StartTime,Description]' \
  --output table
```

**6 — Idle public IPv4 (AWS now charges for *all* public IPv4, attached or not — $0.005/hr ≈ $3.60/mo each)**
```bash
aws ec2 describe-addresses --query 'Addresses[?AssociationId==null].[PublicIp,AllocationId]' --output table
```

**7 — Savings Plans coverage + purchase recommendation (usually the single largest win)**
```bash
aws ce get-savings-plans-coverage --time-period Start=$S,End=$E
aws ce get-savings-plans-purchase-recommendation \
  --savings-plans-type COMPUTE_SP --term-in-years ONE_YEAR \
  --payment-option NO_UPFRONT --lookback-period-in-days SIXTY_DAYS
```
> Compute Savings Plans reach up to ~66% off on-demand and follow you across instance family, region, Fargate and Lambda. EC2 Instance SPs go to ~72% but lock family + region. For a startup whose architecture will change, **recommend Compute SP** and cover only 60–70% of baseline — never 100%.

**8 — NAT Gateway data processing (the sneaky one)**
```bash
aws ce get-cost-and-usage \
  --time-period Start=$S,End=$E --granularity MONTHLY --metrics UnblendedCost \
  --filter '{"Dimensions":{"Key":"USAGE_TYPE_GROUP","Values":["EC2: NAT Gateway"]}}' \
  --group-by Type=DIMENSION,Key=USAGE_TYPE

aws ec2 describe-vpc-endpoints --query 'VpcEndpoints[].[ServiceName,VpcEndpointType]' --output table
```
> High NAT processing **and** no S3/ECR gateway endpoints = you just found four figures a month. Gateway endpoints for S3 and DynamoDB are free.

**9 — CloudWatch log groups with infinite retention**
```bash
aws logs describe-log-groups \
  --query 'logGroups[?retentionInDays==null].[logGroupName,storedBytes]' --output table
```

**10 — RDS: oversized, and Multi-AZ where it shouldn't be**
```bash
aws rds describe-db-instances \
  --query 'DBInstances[].[DBInstanceIdentifier,DBInstanceClass,MultiAZ,AllocatedStorage,Engine]' \
  --output table
```
> Multi-AZ on anything named `staging`/`dev`/`test` is a doubled bill for an environment nobody would notice was down.

**11 — Old-generation instances still running**
```bash
aws ec2 describe-instances --filters Name=instance-state-name,Values=running \
  --query 'Reservations[].Instances[].[InstanceId,InstanceType,LaunchTime,Tags[?Key==`Name`].Value|[0]]' \
  --output table | grep -E 'm3\.|m4\.|c3\.|c4\.|r3\.|r4\.|t2\.'
```
> Newer generations are cheaper *and* faster. Graviton (ARM) delivers roughly 20–40% better price-performance again, and for containerized workloads the migration is often a base-image change and a rebuild.

**12 — Non-prod inventory (the scheduling candidates)**
```bash
aws ec2 describe-instances \
  --filters "Name=tag:Environment,Values=dev,development,staging,test,qa" \
  --query 'Reservations[].Instances[].[InstanceId,InstanceType,State.Name]' --output table
```
> Used ~50 hrs/week, billed 168. Scheduling nights + weekends is a **~70% cut** on everything in this list.

### Open-source tools — the leverage layer

| Tool | Job | When |
|---|---|---|
| [**Cloud Custodian**](https://cloudcustodian.io) | YAML policy engine: find *and* remediate. Write once, reuse per client | The core of your productization — and the retainer's guardrails |
| [**Komiser**](https://github.com/tailwarden/komiser) | Multi-cloud inventory + dashboard | Screenshots for the findings report |
| [**Infracost**](https://infracost.io) | Cost delta per Terraform PR | The *prevention* layer — sell this in the retainer |
| [**OpenCost / Kubecost**](https://opencost.io) | Per-namespace/workload k8s allocation | Any client running EKS |

**This is the "wheel" you asked about.** Compute Optimizer, Cost Optimization Hub, and Trusted Advisor are free and already running inside every client's account, generating recommendations nobody has ever opened. Cloud Custodian is free and open source. You did not invent the wheel — you are the delivery layer that turns it, and that layer is the product.

---

## 4. Risk Management & Liability Shield

### 4.1 The one rule that shields everything

> **You never hold write access to a client's production environment. Not once, not temporarily, not "just for this."**

You propose. They dispose. Every persistent change is a pull request in *their* repository, reviewed and merged by *their* engineer. The git history is the record of shared, documented, timestamped decision-making. This one structural choice does more legal protection than any clause you can draft — and it is also your best sales asset, because it makes the "what if you break prod" objection factually unanswerable.

### 4.2 Change risk tiers

| Tier | Contents | Risk | Approval needed |
|---|---|---|---|
| **T0 — Reversible** | Delete unattached volumes (post-snapshot), set log retention, S3 lifecycle rules, delete snapshots >90d, release idle EIPs | None to running workloads | Email approval |
| **T1 — Financial only** | Savings Plans / RI purchase | Zero technical risk; 1-year commitment | Written CEO/CFO sign-off |
| **T2 — Config change** | Rightsizing, gp2→gp3, instance family migration, RDS class change, disable non-prod Multi-AZ | Restart / brief interruption | PR + agreed maintenance window |
| **T3 — Architectural** | VPC endpoints, k8s autoscaling, Spot adoption, Graviton migration | Highest value, highest care | PR + staged rollout + rollback plan |

**Always sequence T0 → T1 → T2 → T3.** T0 and T1 alone typically deliver 50–60% of total identified savings with zero downtime risk. Bank the trust before you propose anything that restarts a workload.

### 4.3 Implementation rules — non-negotiable

1. **No console clicks for anything persistent.** Terraform / CDK / CloudFormation PRs only.
2. **No IaC at the client?** You supply a script plus a written rollback, and *they* execute it. Or you sell IaC adoption as a separate engagement.
3. **One change per PR.** Never bundle. A bundled PR cannot be cleanly reverted.
4. **Snapshot before any deletion. Always.** Retain 30 days minimum.
5. **T2+ changes only inside a maintenance window agreed in writing.**
6. **Verify after every change** — the next Cost Explorer cycle plus the relevant CloudWatch metric.

### 4.4 PR template — commit this to every client repo

```markdown
## Cost change: [one-line summary]

**Tier:** T0 / T1 / T2 / T3
**Estimated monthly saving:** $X
**Estimated annual saving:** $Y

### What this changes
[Plain-English description of the resource-level change]

### Why it's safe
[Utilization evidence: CloudWatch metrics, time range observed, what confirms
this resource is idle/oversized]

### Blast radius
[Exactly what could be affected if this is wrong]

### Rollback
[Literal commands or `git revert` + apply. Must be executable by someone who
did not write this PR, at 3am, without calling us.]

### Verification
[The metric or Cost Explorer view that confirms success, and when to check it]

---
Proposed by [Agency]. Reviewed and merged by client engineering.
```

That footer line is doing real work. Make it a habit.

### 4.5 Liability language — draft for your lawyer

> **Advisory capacity and client approval.** Consultant acts solely in an advisory capacity. All recommendations are delivered as written proposals or version-controlled pull requests for Client's independent review. Client retains sole authority and sole responsibility for evaluating, approving, testing, merging, and deploying any change. Consultant holds no write, modify, or delete permissions in Client's production environments at any time during the engagement, and no change proposed by Consultant takes effect except through an affirmative act by Client personnel.
>
> **No warranty of savings.** Savings figures are estimates derived from Client's historical usage data as observed during the audit period. Actual realized savings will vary with changes in Client's workload, architecture, traffic, and provider pricing. Consultant makes no guarantee of any specific savings amount.
>
> **Client responsibilities.** Client is responsible for maintaining backups, disaster recovery capability, and adequate pre-production testing, and for validating any change in a non-production environment before deploying it to production.
>
> **Limitation of liability.** To the maximum extent permitted by law, Consultant's total aggregate liability arising out of or relating to this agreement shall not exceed the total fees actually paid by Client to Consultant under this agreement in the [three (3)] months preceding the event giving rise to the claim. In no event shall Consultant be liable for any indirect, incidental, special, consequential, exemplary, or punitive damages, or for any loss of profits, revenue, data, business interruption, or goodwill, whether based in contract, tort, negligence, strict liability, or otherwise, and whether or not Consultant was advised of the possibility of such damages.

**Have a licensed attorney adapt this to your jurisdiction before use.** Limitation-of-liability clauses are enforced very differently across jurisdictions, and some of the above is unenforceable in certain consumer or statutory contexts. Additionally: liability caps get challenged; **E&O insurance actually pays.** Budget roughly $50–150/month for tech E&O coverage and treat it as a cost of doing business, not an optional extra.

### 4.6 Define savings measurement *before* you start

The most common way a gainshare engagement turns into a fight is that nobody defined "savings." Put this in the SOW:

- **Baseline** = trailing 3-month average of the specific cost line items in scope
- **Measurement window** = the 30 days following implementation of each change
- **Realized saving** = baseline minus actual, on in-scope line items only
- **Annualized** = monthly realized × 12
- **Explicitly excluded** — organic growth, workload changes Client made independently, provider price changes, and any line item outside the agreed scope

**Recommendation for clients 1–5: charge a flat fee derived from the estimate, agreed before implementation.** Gainshare sounds better and reads better on a pitch deck, but it requires measurement tooling and legal muscle you don't have yet, and the disputes land exactly when you can least afford them. Move to gainshare at client 6+, once you have both.

---

## 5. Handling Objections

### "Our DevOps team already handles this."

> "I'd expect them to — they're probably better engineers than me. But the question isn't capability, it's calendar. When did they last spend a full uninterrupted week on cost, versus shipping?
>
> Here's the offer: read-only access, 48 hours. If your team already caught everything, you get a written confirmation that your account is clean — which is worth something to you at board time. If I find something they missed, you keep the savings. The only thing at risk is 20 minutes of your time.
>
> And honestly, I'd rather your engineers spend that week on the product. That's the actual return."

**Never disparage their team.** Agree hard, then reframe from competence to opportunity cost.

---

### "We have AWS credits right now, so we don't care."

The most common Series A objection, and the weakest. Take it seriously and dismantle it in order:

> "That's exactly why now is the right time, and I'll explain why in one minute.
>
> **First — when do they expire, and what's your current monthly run rate?**
>
> [They answer. It's almost always 12–24 months out and they haven't done the arithmetic.]
>
> So on [date], you start paying [$X] a month, in cash, for the first time. And here's the pattern I see repeatedly: companies 3–5× their run rate during the credit period, because nothing is pushing back. The habits you build while it's free are the bill you inherit when it isn't.
>
> **Second — credits are finite.** Every dollar of waste is a dollar of credit not spent on growth. Burning them faster just brings the cliff closer.
>
> **Third — waste compounds architecturally.** A bad default costs a day to fix now. After 18 months of propagating across every environment and every new service, it costs a quarter.
>
> I'm not selling you a lower bill this month. I'm selling you not having a crisis in month fourteen."

---

### "What if you break something in production?"

Answer structurally, not with reassurance:

> "I can't, and that's by design rather than by promise.
>
> **I don't have write access, and I'm not asking for any.** My access is read-only, and I'll send you the exact IAM policy before you create the role — it explicitly excludes reading your S3 objects, your Lambda code, your logs, and your secrets.
>
> **Every change is a pull request in your repo.** Your engineer reviews and merges it. I never merge anything. One change per PR, with the rollback procedure written into the description.
>
> **Nothing gets deleted without a snapshot first.**
>
> **Every API call I make appears in your CloudTrail** — you can audit exactly what I touched, in real time, without asking me.
>
> And phase one is entirely non-disruptive anyway: orphaned storage, log retention, and Savings Plans. None of that touches a running workload. We only discuss anything that restarts a service after you've already seen me be right about the easy stuff."

---

### Bonus: "You're 21. Why should I trust you with this?"

You will get this. Do not dodge it.

> "Fair. Here's what I'd do in your position: don't trust me — verify me.
>
> The access is read-only and you revoke it with one click. The findings doc is checkable line-by-line against your own Cost Explorer. And you don't pay until you've seen a number you agree with.
>
> There's no version of this where I get paid before you get value. If the work is bad, you'll know before any money moves."

The structure of the offer *is* the credibility. Lean on it instead of trying to sound older.

---

### Bonus: "Send me a proposal / let me think about it."

The polite no. Collapse it back to the only real decision:

> "Sure — though the proposal is one page and it's identical for everyone: read-only role, 48 hours, findings doc, no charge under $2k. The only actual decision is whether to create the role. Want me to send the CloudFormation template, and you decide once you've seen the number?"

---

## 6. The Recurring Retainer Upsell

### 6.1 Why re-bloat is real

Cost optimization is not a project, it's a **leak**. Every sprint adds resources. Every new environment, every test cluster, every "temporary" instance that outlives its author. With no control loop, an optimized account degrades most of the way back to baseline in **6–9 months**.

You can prove this to them with their own data: pull 24 months of Cost Explorer and show them their historical drift. They will recognize the shape immediately.

### 6.2 The transition moment

Pitch it **in the findings-review call**, right after they've seen the number — not in a separate meeting a month later. The emotional peak is the moment they see $54,000/year on a slide.

> "One thing about this list — roughly 60% of it comes back. Not because anyone's careless, just because nobody owns the bill. Want me to make sure it doesn't?"

### 6.3 What's actually in it

Tangible artifacts only. Never sell "monitoring."

- **Monthly cost review** — written report + 30-minute call
- **AWS Cost Anomaly Detection** configured, monitored, and triaged by you (the service is free; the triage is the product)
- **Infracost in their CI** so every PR shows its cost delta *before* merge — the prevention layer
- **Cloud Custodian guardrails**, maintained: mandatory tagging, scheduled non-prod shutdown, auto-cleanup of unattached volumes after N days, snapshot lifecycle enforcement
- **Savings Plan coverage management** — re-optimize as commitments expire and usage grows
- **Budget alerts + monthly forecast vs. actual**
- **Quarterly deep audit** — the full sweep, rerun
- **Slack channel access** for "is it cheaper to do X or Y" architecture questions
- **A one-page monthly artifact the CTO can forward straight to the CEO or the board**

That last item is the real product. **The CTO looks good in front of their board every single month, because of you.** That is why retainers renew, and it has almost nothing to do with the technical work.

### 6.4 Pricing tiers

| Tier | Price | Includes | Your time |
|---|---|---|---|
| **Watch** | $1,500/mo | Monthly report, anomaly alerts, budget management, Slack access | ~4 hrs |
| **Guard** | $3,000/mo | + Custodian guardrails maintained, Infracost in CI, SP management, quarterly deep audit | ~10 hrs |
| **Embedded** | $5,000+/mo | + implementation hours included, k8s optimization, architecture cost review | ~20 hrs |

**The pricing rule:** keep the retainer **under 10% of the monthly savings it protects.** Save them $6k/mo and $1,500/mo approves itself in one sentence. If you only saved them $3k/mo, do not pitch $3k/mo — pitch Watch, or don't pitch at all.

### 6.5 The close

> "The audit gets the money back once. The retainer is what keeps it from growing back."

Then remove the friction: **include the first month of the retainer in the audit fee.** They're onto the recurring rail with no second decision to make, and you have 30 days to make cancelling feel like a downgrade.

### 6.6 Why this is the whole business

**10 retainers × $2,000 = $20,000/month for roughly 40–60 hours of work.** Audits are how you acquire; retainers are the asset. Audit-only revenue is a job with extra steps — you re-earn it from zero every month. Retainer revenue compounds, and every retained client is simultaneously a reference and a referral source.

---

## 7. Team assignments

| Person | Owns | Why |
|---|---|---|
| **You (DevOps)** | Delivery: audits, findings docs, PRs, tooling | The only one who can do the technical work today |
| **Marketing + law friend** | Outreach volume, calls, closing, MSA/SOW, NDA handling | **The highest-leverage role.** Deal flow is the constraint, not delivery |
| **Cyber friend** | Second delivery capacity; the security-review upsell later | Trains on audits now; SOC2-prep upsell is phase two |
| **Electrician friend** | Not this play | Real option later: trades businesses are the most underserved software market alive, and he has native access |

**Critical warning for the cyber friend:** never scan, probe, or test any client system without **explicit written authorization defining the scope**. Unauthorized access and scanning is a criminal offence in most jurisdictions regardless of intent, and "we were demonstrating a vulnerability" is not a defence. The cost audit uses read-only API calls under a role the client created — that is authorized and fine. Anything resembling a penetration test needs a separate signed scope-of-work first. This is exactly what your law friend is for.

---

## 8. The first 14 days

| Days | Action | Owner |
|---|---|---|
| 1 | Write the offer in 3 sentences. Register domain + email. Draft NDA/SOW to lawyer for review | Marketing/law |
| 1–2 | Build the 100-company list. Do not send anything yet | All four |
| 2 | Build the CloudFormation template for the read-only role. Test the 12 commands end-to-end in your own AWS account | You |
| 3–4 | **Warm network first.** Every contact any of you has at a company with cloud infrastructure. Ask for the intro directly | All four |
| 5–10 | Cold outreach: 20 touches/day, email + LinkedIn. Run discovery calls as they land | Marketing/law |
| 8–12 | First audit delivered: 48-hour turnaround, findings doc, review call | You + cyber |
| 12–14 | Invoice same day as delivery. Ask for the testimonial and one referral in the same message. Pitch the retainer on the review call | Marketing/law |

**Spend $0 on:** ads, courses, logos, a website, incorporation before you have a client who wants to pay you. The first $1,000 requires a spreadsheet, an email account, and an AWS account you already have.

---

## 9. What kills this business

1. **Doing research instead of outreach.** The list is not the work. The sending is the work.
2. **Building the tooling before client #3.** You do not know what to automate yet. Do it manually three times, painfully, then automate exactly what hurt.
3. **Taking a client with a $3k/month bill.** The fee can't justify the hours. Say no.
4. **Gainsharing without a measurement clause.** The invoice becomes an argument and you lose both the money and the reference.
5. **Getting write access "just this once."** The moment you have it, every outage in that account is a conversation about you.
6. **Selling reports instead of implementation.** Every CTO already knows they're wasting money. Knowing is worthless. Doing is the product.
7. **Stopping at the audit.** One-off revenue means you start from zero every month. The retainer is the business.

---

## Sources

- [AWS: Access to AWS accounts owned by third parties](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_common-scenarios_third-party.html)
- [AWS: Identity-based policies for AWS Cost Management](https://docs.aws.amazon.com/cost-management/latest/userguide/billing-permissions-ref.html)
- [AWS: CostOptimizationHubReadOnlyAccess managed policy](https://docs.aws.amazon.com/aws-managed-policy/latest/reference/CostOptimizationHubReadOnlyAccess.html)
- [AWS Billing policy examples](https://docs.aws.amazon.com/awsaccountbilling/latest/aboutv2/billing-example-policies.html)
- [DoiT: AWS Savings Plans vs Reserved Instances 2026](https://www.doit.com/blog/aws-savings-plans-vs-reserved-instances-2026-the-decision-guide-engineers-actually-need)
- [Spendbase: Open Source Cloud Cost Optimization Tools 2026](https://www.spendbase.co/blog/cloud/open-source-cloud-cost-optimization-tools-2026-10-top-picks-for-aws-gcp-and-kubernetes/)
- [CloudFix: AWS Cost Optimization Tools Compared 2026](https://cloudfix.com/blog/aws-cost-optimization-tools-comparison/)
- [Cloud Custodian](https://cloudcustodian.io) · [Komiser](https://github.com/tailwarden/komiser) · [Infracost](https://infracost.io) · [OpenCost](https://opencost.io)
