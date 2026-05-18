# Nitpick Agent Context

nitpick-agent is the reusable runtime behind Nitpick-style code review workflows. This file captures the domain language that architectural work should reuse consistently.

## Language

**Review request**:
A request for nitpick-agent to review a pull request from a review source.
_Avoid_: PR job, review task

**Review source**:
The source that discovers review requests and prepares review input for the runtime.
_Avoid_: provider, service

**Artifact sync destination**:
The destination that syncs locally stored review artifacts outward after the runtime produces them.
_Avoid_: publisher, exporter

**GitHub review workflow**:
The full GitHub pull-request flow: discovery, preparation of review input, sync of review artifacts, and checkout cleanup.
_Avoid_: GitHub service, GitHub integration layer

## Relationships

- A **Review source** discovers a **Review request**
- A **Review request** produces review artifacts that an **Artifact sync destination** can sync outward
- The **GitHub review workflow** owns GitHub-specific discovery, preparation, sync, and cleanup for a **Review request**

## Example dialogue

> **Dev:** "Should the **GitHub review workflow** prepare the checkout before the runtime starts the review?"
> **Domain expert:** "Yes — checkout management is part of the **GitHub review workflow** implementation, not something callers should coordinate outside the module."

## Flagged ambiguities

- "GitHub integration" was too vague — use **GitHub review workflow** when the concept includes discovery, preparation, sync, and cleanup.
