# Signing orx.exe

Windows builds are unsigned, so `orx.exe` and the PowerShell installer both trip
SmartScreen. This is the setup runbook for fixing that with [Azure Artifact
Signing](https://learn.microsoft.com/en-us/azure/trusted-signing/), the service
Microsoft renamed from Trusted Signing. `dist` supports it from 0.33.0, and
`cargo-dist-version` is currently 0.32.0, so **the version must be bumped first**.

## What signing does and does not buy

Signing replaces "Unknown publisher" with alphaXiv and lets SmartScreen
reputation accumulate across releases. It does not silence the first prompt.
Three things, before anyone spends money:

- **EV certificates no longer bypass SmartScreen.** That behavior was removed in
  2024; an EV cert now builds reputation exactly like an OV one, at triple the
  price. Do not buy EV for this.
- **A self-signed "dev" certificate is worse than no signature.** Windows does
  not trust it, so users get an untrusted-signature block rather than an
  unknown-publisher one. Self-signed is for local testing, or for fleets whose IT
  pushes the root via Intune or Group Policy.
- **Reputation is per signing identity.** Changing certificates later restarts
  it, so this should be an identity alphaXiv keeps.

## Why this service

Azure costs about $9.99/month against $150–300/year for an OV certificate, and
none of the paid options differ on SmartScreen — but price is the least of it.
`dist` generates the signing steps, so there is no bespoke job to maintain.
Authentication is GitHub OIDC, so **no key material lives in GitHub secrets** —
unlike the macOS pipeline, which has to hold `MACOS_CERT_P12_BASE64` (see
`macos/DISTRIBUTION.md`). And since June 2023 the CA/Browser Forum has required
code signing keys to sit on an HSM, so a traditional CA means a USB token or a
second cloud signing subscription anyway.

[SignPath Foundation](https://signpath.org/terms.html) signs qualifying
open-source projects free, but the publisher string reads *SignPath Foundation*,
so reputation accrues to them rather than to alphaXiv.

## Set it up

Public Trust certificates require an organization in one of about a dozen
jurisdictions
([current list](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart));
alphaXiv qualifies as a US entity.

**The critical path is identity validation: Microsoft takes 1 to 20 business
days**, longer if it asks for more documents. Start it first.

The names below are examples, and several are reused verbatim later — a mismatch
is likely, because the account name may well be taken. The resource group
(`orx-signing`) and account name (`alphaxiv`) are chosen in step 1 and recur in
both of step 3's commands; the account name also appears in step 4's config. The
profile name (`orx-release`) is chosen in step 3's first command and recurs in
its second and in step 4's config. The region picked in step 1 determines step
4's `endpoint`.

### 1. Azure account and identity validation

You need Contributor on the subscription to create the account, and the
portal-only **Artifact Signing Identity Verifier** role — which additionally
requires at least Reader at subscription scope — to submit the validation.

```bash
az provider register --namespace Microsoft.CodeSigning
# Registration is asynchronous; wait for "Registered" before continuing.
az provider show -n Microsoft.CodeSigning --query registrationState
az extension add --name artifact-signing
az group create --name orx-signing --location eastus
az artifact-signing create -n alphaxiv -l eastus -g orx-signing --sku Basic
```

The account name is globally unique across all Azure tenants, 3–24 alphanumeric
characters starting with a letter, so have a fallback ready if `alphaxiv` is
taken.

Now the irreversible part. **A submitted identity validation cannot be amended.**
Any wrong value — legal entity name, address, website domain — means cancelling
and starting a fresh 1-to-20-day wait. The entity name also becomes the publisher
string users see in place of "Unknown publisher", and per the reputation rule
above, correcting it later resets reputation. Both email addresses must be
monitored mailboxes on a domain the entity owns and must accept external links:
verification links expire in seven days, and a failed email check also means
starting over.

So: confirm the exact registered legal name, address, and domain **before**
opening the form. Then submit it in the portal — the CLI cannot do this step —
under the account's **Identity validations** → **Organization** → **New
Identity** → **Public**.

### 2. GitHub identity and environment

This step depends on nothing else — do it while validation runs.

Create an Entra app registration and give it a federated credential for this
repository.

**The credential entity type is Environment, not Branch or Tag**: dist puts the
signing jobs in a GitHub environment, so the OIDC subject is

```
repo:alphaXiv/OpenResearch:environment:release
```

with audience `api://AzureADTokenExchange`. A `refs/tags/*` subject cannot work
here — `dispatch-releases = true` means releases run on `workflow_dispatch` and
no workflow ever pushes a tag, so no token is ever minted on a tag ref. Prefer
the portal's credential builder over hand-writing the subject; repositories
created after 15 July 2026 default to an immutable subject format carrying owner
and repository IDs.

Create a GitHub environment named `release` and add `AZURE_CLIENT_ID`,
`AZURE_TENANT_ID`, and `AZURE_SUBSCRIPTION_ID` to it as environment secrets.
Both jobs that read them declare `environment: release`, so environment secrets
scope tighter than repository ones, matching what the macOS pipeline does with
`release-signing`.

Creating the environment explicitly is about control, not breakage: GitHub
silently creates an environment a workflow references but that does not exist, so
skipping this yields a release that works and an environment with no protection
rules. Note that `environment:` sits on the whole `build-local-artifacts` job,
outside its matrix — so required reviewers there gate the macOS and Linux builds
too. That is the same trade the macOS pipeline makes, where the reviewer
approving the run is the real gate on the certificate — expect two prompts per
release, since both jobs enter the environment.

### 3. Certificate profile and signing rights

**Blocked on identity validation completing, and on the app registration from
step 2.** Creating the role assignment also needs
`Microsoft.Authorization/roleAssignments/write` — Owner or User Access
Administrator, which Contributor from step 1 does not include.

Create the profile against the validation ID from the portal:

```bash
az artifact-signing certificate-profile create -g orx-signing \
  --account-name alphaxiv -n orx-release --profile-type PublicTrust \
  --identity-validation-id <id from the portal>
```

Then grant the app's **service principal** signing rights on the profile alone,
not the whole account. The principal's object id is not the app registration's
object id, and an app created with `az ad app create` rather than the portal has
no principal at all until `az ad sp create --id <app client id>` makes one:

```bash
# The object id the role assignment needs:
az ad sp show --id <app client id> --query id -o tsv

az role assignment create \
  --assignee-object-id <service principal object id> \
  --assignee-principal-type ServicePrincipal \
  --role "Artifact Signing Certificate Profile Signer" \
  --scope "/subscriptions/<sub>/resourceGroups/orx-signing/providers/Microsoft.CodeSigning/codeSigningAccounts/alphaxiv/certificateProfiles/orx-release"
```

Before moving on, confirm both landed — neither check touches CI or signing
quota:

```bash
az artifact-signing certificate-profile show -g orx-signing \
  --account-name alphaxiv -n orx-release --query status
az role assignment list --scope "<the profile scope above>" -o table
```

The profile should report `Succeeded`, and the listing should show the service
principal against the signer role.

### 4. Turn it on

**Do not start this step until steps 1 through 3 are complete and verified.**
`dist` has no runtime gate equivalent to the macOS job's `MACOS_SIGNING_ENABLED`
variable: once the config is present, every release build tries to sign, and a
missing account or role assignment fails the release rather than skipping it. A
dry run (`release.yml` dispatched with `tag=dry-run`) is not a safe rehearsal —
it builds local artifacts and runs `sign-artifacts`, so it will attempt
`azure/login` and consume signing quota. That is why this ships as an instruction
rather than a commented-out block.

To stop signing later, delete the config block and re-run `dist generate`; that
is enough to keep release builds from touching Azure. Abandoning it properly also
means removing the role assignment, the federated credential, the `release`
environment's secrets, and the resource group — the last one because the
subscription keeps billing until it goes. Keep the validated identity unless you
are sure: re-validating later restarts SmartScreen reputation from zero.

Do these in order. Adding the config block without the version bump is the thing
to avoid: 0.32.0 ignores the block silently rather than erroring, which would
ship unsigned binaries with no failure anywhere.

1. Install dist 0.33.0. If it is not yet on crates.io, take the upstream tag:

   ```bash
   cargo install --git https://github.com/axodotdev/cargo-dist --tag v0.33.0 cargo-dist --locked
   ```

   Only the local install needs this — CI installs dist from its GitHub release
   installer, never from crates.io.

2. Set `cargo-dist-version = "0.33.0"` in `dist-workspace.toml`.

3. Add the config block to `dist-workspace.toml`, with the endpoint for the
   account's region (the
   [quickstart](https://learn.microsoft.com/en-us/azure/trusted-signing/quickstart)
   carries the region table; `eastus` is `https://eus.codesigning.azure.net`). It
   must be workspace-level — `dist` warns and ignores it under
   `package.metadata.dist`:

   ```toml
   [dist.azure-windows-sign]
   endpoint = "https://eus.codesigning.azure.net"
   account-name = "alphaxiv"
   certificate-profile-name = "orx-release"
   ```

4. Run `dist generate` and commit both files. Confirm the regenerated
   `release.yml` gained a `sign-artifacts` job, that `id-token: write` appears in
   the top-level `permissions` block, and that the `custom-ci` job is still there
   — `AGENTS.md` requires `./ci` stay in `global-artifacts-jobs`.

5. Merge. That alone releases nothing — in this repo the release act is merging
   a *second* PR that bumps `version` in `Cargo.toml`, which is what dispatches
   `release.yml` (see `.github/workflows/release-on-bump.yml`). Do not dispatch
   `release.yml` by hand: its `tag` input defaults to `dry-run`, and it would
   build from main's tip rather than the bump commit.

   Then verify the result rather than assuming it — a release that merely does
   not fail cannot be distinguished from one that silently skipped signing. On a
   Windows machine, against the downloaded `orx.exe`:

   ```powershell
   Get-AuthenticodeSignature .\orx.exe |
     Format-List Status, SignerCertificate, TimeStamperCertificate
   ```

   `Status` should be `Valid`, `SignerCertificate`'s subject should be the
   validated legal entity, and **`TimeStamperCertificate` must not be empty**.
   Azure's signing certificates live about three days, so the countersignature is
   the only thing keeping released binaries valid past that; without it every
   artifact goes invalid within the week. `dist` timestamps automatically against
   `timestamp.acs.microsoft.com`, so an empty value means something went wrong.
   Check the `.ps1` installer the same way.

## What gets signed

`orx.exe`, signed on the Windows leg of `build-local-artifacts`, and
`openresearch-cli-installer.ps1`, signed in the `sign-artifacts` job that runs
after the global artifacts build. Signing the installer does nothing for the
`irm … | iex` flow in `docs/windows.md`, which never evaluates the signature, but
it does cover the saved-and-run case and hosts under an `AllSigned` policy.

`dist` signs x86_64 Windows only — the repo's sole Windows target. It errors
rather than skipping if an aarch64 Windows target is ever added, if both signing
backends are set at once, or if a field under `azure-windows-sign` is missing or
blank.

Nothing about self-update changes. `src/updates/windows.rs` swaps the binary by
rename and checks no signature, unlike `src/updates/macos_app.rs`, which pins a
Developer ID team and carries its own rotation warning in
`macos/DISTRIBUTION.md`.

Details above about dist's generated workflow and the `az` command surface were
read from dist 0.33.0 and Microsoft's docs as of September 2026; re-check them
against your own `dist generate` output and the current quickstart.
