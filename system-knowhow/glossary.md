---
name: Lucidos Glossary
description: Canonical user-facing terms: app, artifact, intent, knowhow, plugin, script, thread, trigger, workspace, event, and the coding-agent ones (Apply, change, Claude Code, coding agent, hardening). Load when a term is unclear or a concept has two names.
---

<!-- domain-event: BuildSlotAcquired: The CLI emits this domain event when a contended host build acquires a slot. -->
<!-- domain-event: BuildSlotWaiting: The CLI emits this domain event when a host build must wait for a slot. -->
<!-- domain-event: JobListingFound: Example domain event emitted by a job-listing integration. -->
<!-- domain-event: PanasonicHeatpumpAdjusted: Example domain event emitted by a heat-pump integration. -->

<!-- domain-event: BuildSlotReleased: The CLI emits this domain event when a host build slot is released. -->
<!-- domain-event: MorningRoutineCompleted: Example domain event emitted by a morning-routine trigger. -->
<!-- domain-event: SetupInterviewCompleted: The setup-interview knowhow emits this domain event when the interview finishes. -->
<!-- domain-event: PluginSetupCompleted: Plugin setup emits this domain event after completing its procedure. -->

# Lucidos Glossary

Canonical definitions for the words Lucidos uses with the user: one term, one place, one definition. Other prose (`system-knowhow/*.md`, the engine system prompt, app/trigger intents, UI strings) uses *this* meaning, never a synonym.

Use the canonical word, not a near-synonym: *sub-thread* for any descendant (not *child thread*), *intent* (not *task*), *knowhow* (not *recipe*), *artifact* (not *attachment*). Add a missing concept in the same change that introduces it.

Two sections: **Core** terms most users meet, and **Advanced (coding agents)**: Claude Code, worktree-based code changes, the Apply / Discard flow, hardening.

This file is the **base layer**. `docs/glossary.md` adds dev-only terms (aggregate, actor, ActorMode, EventBus, BusEvent, ThreadEvent, projection, worktree, …) that the workspace LLM doesn't need.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

### `system-knowhow/glossary/core-terms`

Core terms.

- Active device
- Active (thread state)
- Affected trigger
- Agent archive
- Agent browser
- Agent sandbox
- Private path
- Archive all
- Action blocker
- App
- App icon
- App-icon badge
- App lint
- App manifest
- App storage
- App UI
- API caller
- Artifact
- Auth module
- Blocking descendant
- Attention-needing descendant
- Canvas
- Build slot
- Capacity policy
- Cascading archive

### `system-knowhow/glossary/core-terms-2`

Core terms (continued).

- Chat thread
- Home thread
- Child follow-up
- Held message
- Queued card
- Held idle proposal
- Child thread
- Command guard
- Command checkpoint
- Command permission card
- Compose destination
- Config
- Connected account
- Connected-but-hidden
- Content pane
- Conversation
- Current section
- Context window
- Credential

### `system-knowhow/glossary/core-terms-3`

Core terms (continued).

- Secret mention
- Secret reference
- Backup recipient
- Backup key
- Backup origin
- Cross-gateway link
- Dashboard
- Dashboard panel
- App shortcut
- Draft
- Draft revision
- Disabled tool
- Derived provider
- Delete (a thread)
- Domain event
- Dynamic bars
- Endpoint catalog
- Derived proxy entry
- Engine
- Environment variable
- Event
- File preview modal

### `system-knowhow/glossary/core-terms-4`

Core terms (continued).

- Find bar
- Font catalog
- Form request
- Event address
- Image handle
- Image size hint
- Imported
- In-app surface
- Intent
- Knowhow
- Knowhow doc
- Knowhow reference
- Last used device
- Live child
- Live co-creation
- Loaded knowhow
- Local certificate
- Lucidos Agent
- Lucidos Engine
- Managed certificate
- Marketplace
- Max tool calls
- MCP server
- Enabled MCP server
- MCP permission card
- Memory choice

### `system-knowhow/glossary/core-terms-5`

Core terms (continued).

- Memory module
- Memory recall
- Memory search
- Model registry
- Vision flag
- Monogram tile
- Retired model
- Served model
- Price table
- Motion
- Move to top level
- Ready signal
- Read marker
- Reasoning effort
- Default effort
- Response style
- Theme
- Theme effects
- Theme family
- Theme part
- Theme token catalog
- Theme mode
- Technical literacy
- Style library
- Preferred provider
- Billing attribution

### `system-knowhow/glossary/core-terms-6`

Core terms (continued).

- Provider
- Claude gateway
- Gateway managed settings
- Gateway usage telemetry
- ChatGPT plan provider
- System One model
- Builtin provider proxy
- Rejected proxy entry
- OAuth provider registry
- OAuth redirect URI
- Web redirect URI
- OAuth client type
- OS surface
- Orchestrator
- Device
- One-off device
- Pairing label
- Paired device

### `system-knowhow/glossary/core-terms-7`

Core terms (continued).

- User
- Team workspace
- Gateway event log
- Pairing code
- Pairing secret
- Pairing request
- Public listener
- Pairing address
- Gateway pairing
- Gateway pairing secret
- Manage grant
- Control grant
- Gateway switcher
- Restore upload
- Return access grant
- Trusted pairing
- Remote gateway
- Remote start setting
- Remote workspace
- External address
- Parent thread
- Part token
- Thread pause
- Pinned thread
- Custom folder
- Plugin
- Plugin category
- Plugin manifest
- Engine requirement
- Plugin local patch
- Plugin media
- Plugin detail page
- Plugin widget

### `system-knowhow/glossary/core-terms-8`

Core terms (continued).

- Plugin modified state
- Plugins panel
- Proxy
- Preference
- Pseudo-fullscreen
- Pull to refresh
- Pane swipe
- One sound at a time
- PresenceCheck
- PresencePong
- Presence outcome
- Shared SSE holder
- NativePushRequested
- Scratch
- Spend cap hold
- Script
- Setup interview
- Setup thread
- Setup revision
- Coding-agent grant
- Program grant
- Folder grant

### `system-knowhow/glossary/core-terms-9`

Core terms (continued).

- Trigger deny
- Signer manifest
- Slowness warning
- Skipped slot
- Source event
- Spawning thread
- Stopped thread
- Stopped child
- Standing deny
- Status filter
- Superseded question
- Protected surface
- Style override
- Style remote
- Sub-thread
- Thread
- Event wait
- Waiting indicator
- Grouping button
- Folders grouping
- Ongoing grouping
- In flight
- Blocked
- Root thread

### `system-knowhow/glossary/core-terms-10`

Core terms (continued).

- Gate
- Driver
- Watcher
- User mention
- Agent mention
- Agent pause
- User-only message
- Notification audience
- Read state
- Config change
- Factory app
- Translation script
- Review
- Read decision
- Wake mode
- Wake decision
- Filed report
- Read request
- Thread link
- Settings link
- Reader fields
- Thread liveness

### `system-knowhow/glossary/core-terms-11`

Core terms (continued).

- Thread drawer
- Focused thread
- Thread pane
- View menu
- Thread Queue
- Title style
- Thread triage
- Thread summary
- Self-curated context mode
- Context panel
- Round stub
- Body
- Handle
- Keep
- Context ledger
- Scratchpad
- Working understanding
- Swept window
- Todo notes
- Todo item
- Todo list

### `system-knowhow/glossary/core-terms-12`

Core terms (continued).

- Top-thread
- Standing instruction
- Owner approval
- Permission request
- Batch
- Permission
- Action
- Trigger
- Interrupted run
- Off-schedule run
- Trigger definition
- Definition digest
- Event subscription
- Field path
- Continue key
- Actor
- Trigger thread
- Trigger group
- Event hook
- Hook definition
- Hook answer
- Software factory
- Stage map
- Typed answer
- Unattested caller
- Unsent message
- Update channel
- Custom slot

### `system-knowhow/glossary/core-terms-13`

Core terms (continued).

- Update address
- Urgent follow-up
- Call toggle
- Voice session
- Wake question
- Quiet option
- Quiet pick
- Quiet reply
- Release notice
- Notice item
- What's New
- System attention badge
- Webhook
- Delivery deduping

### `system-knowhow/glossary/core-terms-14`

Core terms (continued).

- Ingress probe
- Refusal check
- Widget
- Reusable widget
- Inline widget
- Widget box
- Widget fit check
- Widget shape
- Widget params
- Widget embed
- Widget instance
- Widget size
- Built-in widget
- Workspace
- Workspace order
- Workspace origin
- Machine-made workspace
- Workspace broker
- Ephemeral workspace
- Trash
- Workspace address
- Workspace font
- Workspace audit
- Audit section
- Workspace prompt footprint
- System prompt footprint

### `system-knowhow/glossary/core-terms-15`

Sections include Core terms (continued) and Advanced (coding agents).

- Auto-start
- Apply
- Start failure
- New version available / Switch to new version
- Build log
- Diagnostic log
- Diagnostic bundle
- Diagnostic drop
- Report code
- Diagnostic inbox
- Internal fault
- Host log
- Host-log transfer
- Apply-ready
- Apply All
- Landing lane
- Landing setting
- Lander
- Origin-first landing
- Follow main
- Build deferral

### `system-knowhow/glossary/advanced-coding-agents`

Advanced (coding agents) (continued).

- Active coding thread
- Landing nudge
- Lane batch
- Arrival hold
- Lane thread
- Main check
- Main-check setting
- Main repair
- Fix thread
- Flaky fix thread
- Apply estimate
- Standing apply
- Settling thread
- Cancel (Stop)
- Change
- Set-aside change
- Change state
- Unproposed work
- Not ready

### `system-knowhow/glossary/advanced-coding-agents-2`

Advanced (coding agents) (continued).

- Successor session
- Provider block
- Successor thread
- Build chain
- Handoff consent
- Handoff estimate
- Handoff checkpoint
- Implementer
- Default implementer
- Intelligence ladder
- Rung control
- Intelligence rung
- Default floor
- Risk rung
- Band
- Intelligence profile
- Ladder surface
- Long-prompt share
- Auto-compact trial
- Successor card
- Context ceiling
- Handoff brief
- Handoff narrative

### `system-knowhow/glossary/advanced-coding-agents-3`

Advanced (coding agents) (continued).

- Integrated sub-thread
- Withdrawn change
- Incomplete change
- Change summary
- Claude Code
- Codex
- Coding agent
- Coding-agent branch
- Coding-agent permission card
- Coding-agent permission mode
- Background task
- Polled background task
- Cache keep-alive
- Reread cap
- Expected wake
- Expected wake decision
- Permission answer
- Coding-agent thread
- Side question
- Side-question mode
- External-repo coding-agent thread

### `system-knowhow/glossary/advanced-coding-agents-4`

Sections include Advanced (coding agents) (continued) and Core terms (continued).

- App coding-agent thread
- WIP app preview
- Hardening
- Hold
- Thread on hold
- On hold section
- Resume
- Repository
- Plan kind
- Design plan
- Workspace snapshot
- Snapshot handle
- Recovery snapshot
- Snapshot restore
- Restore journal
- Poster

## When to add a term

Define new user-facing terms in a glossary part and add one bullet under that
part's heading in this index. Put coding-agent-only terms in an **Advanced
(coding agents)** section. Define dev-only terms in `docs/glossary.md`.

## When a term changes

If a term is renamed, retired, or shifts meaning, update its part and keep the
index bullet aligned in the same commit. Update affected knowhow files as
described in `.claude/rules/system-knowhow.md`. `/harden` checks for drift between
the glossary and the product.
