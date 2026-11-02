"""Local source, scope and interval contracts; no provider calls or benchmark scoring."""

import asyncio

import httpx
import pytest
from nous_sdk import NousClient, NousError
from nous_sdk.contracts import (
    AdmitFiles,
    AdoptCognition,
    DesireBody,
    DesiredCondition,
    DomainKnowledgeBody,
    EgoFacetBody,
    EntityBody,
    EpistemicStance,
    EvidenceLink,
    ImportDocument,
    LanguageConventionBody,
    MediaReference,
    SocialRelationBody,
    SourceCommand,
    TimePoint,
    TimeRange,
)
from nous.platform.call_context import CallContext
from nous_sdk.contracts.context import ContextPrepare, ContextUsage
from nous_sdk.contracts.facts import ChatFactScopeInput, FactChange, FactClaimInput
from nous_sdk.contracts.motivation import AssessMotivation, MotivationAssessmentsQuery
from nous_sdk.contracts.namespace_backups import RestoreNamespaceBackup
from nous_sdk.contracts.personality import RelationshipRequirement
from nous_sdk.contracts.social import SymbolSelection

from nous.bootstrap import application
from nous.platform.configuration import Settings
from nous.transports.http import create_app


def test_personality_supply_and_symbols_respect_source_and_social_scope(tmp_path):
    async def scenario():
        async with application(Settings(instance=tmp_path / "instance")) as app:
            ctx = CallContext(
                caller_id="owner",
                namespace_id="ns",
                subject_id="subject",
                credential_source_ids=None,
            )
            await app.identity.create_namespace(ctx, "Namespace", "ns")
            await app.identity.create_subject(
                ctx,
                "Subject",
                "subject",
                subject_id="subject",
                capabilities=("memory", "ego", "social"),
            )
            source = await app.imports.admit_files(
                ctx,
                AdmitFiles(
                    operation_id="source",
                    inputs=(ImportDocument(text="Explicit creator and conversation evidence."),),
                    llm_enabled=False,
                    vectors="disabled",
                ),
            )
            evidence = (EvidenceLink(reference=source.outputs[0].cognition[0]),)

            async def adopt(body, identity):
                return await app.cognition.adopt(
                    ctx, AdoptCognition(operation_id=identity, body=body, evidence=evidence)
                )

            identity = await adopt(
                EgoFacetBody(
                    facet="identity",
                    description="A sourced fictional repairer.",
                    category="identity",
                ),
                "identity",
            )
            conditional = await adopt(
                EgoFacetBody(
                    facet="humor", description="A friendly private joke.", contexts=("familiar",)
                ),
                "conditional",
            )
            friend = await adopt(EntityBody(label="Friend", entity_type="person"), "friend")
            group = await adopt(EntityBody(label="Development", entity_type="group"), "group")
            other = await adopt(EntityBody(label="Other", entity_type="group"), "other")
            actor = await adopt(EntityBody(label="Subject", entity_type="person"), "actor")
            relational = await adopt(
                EgoFacetBody(
                    facet="scoped_tendency",
                    category="tendency",
                    description="A relationship-scoped tendency.",
                    contexts=("familiar",),
                    relationship=RelationshipRequirement(
                        from_entity=actor.reference,
                        dimension="familiarity",
                        minimum_degree=0.5,
                        minimum_confidence=0.7,
                    ),
                ),
                "relational",
            )
            knowledge = await adopt(
                DomainKnowledgeBody(
                    domain="compilers", understanding="An explicitly bounded topic."
                ),
                "knowledge",
            )
            carrier = MediaReference(
                kind="material",
                id=source.catalog.units[0].material_id,
                observation_id=source.outputs[0].observation_id,
            )
            convention = await adopt(
                LanguageConventionBody(
                    expression="smoke",
                    meaning="A build failed.",
                    audience=(group.reference,),
                    groups=(group.reference,),
                    pragmatics="comfort",
                    intentions=("comfort",),
                    carriers=(carrier,),
                    avoid_for=(friend.reference,),
                ),
                "convention",
            )
            understood = await app.symbols.understand(
                ctx,
                SymbolSelection(
                    expression="smoke", audience=(friend.reference,), groups=(group.reference,)
                ),
            )
            assert tuple(record.reference for record in understood.candidates) == (
                convention.reference,
            )
            assert (
                await app.symbols.understand(
                    ctx, SymbolSelection(expression="smoke", groups=(other.reference,))
                )
            ).unknown
            assert (
                await app.symbols.expressions(
                    ctx,
                    SymbolSelection(
                        intent="comfort", audience=(friend.reference,), groups=(group.reference,)
                    ),
                )
            ).unknown
            prepared = await app.contexts.prepare(
                ctx,
                ContextPrepare(
                    operation_id="context",
                    session_id="one",
                    consumer_id="default",
                    instance_id="test",
                    usage=ContextUsage(purpose="reply", view="subject"),
                ),
            )
            references = {
                target.reference
                for segment in prepared.response.projection.segments
                for target in segment.sources
            }
            assert identity.reference in references and conditional.reference not in references
            assert knowledge.reference not in references
            familiar = await app.contexts.prepare(
                ctx,
                ContextPrepare(
                    operation_id="familiar",
                    session_id="two",
                    consumer_id="default",
                    instance_id="test",
                    usage=ContextUsage(purpose="reply", view="subject", situations=("familiar",)),
                ),
            )
            assert conditional.reference in {
                target.reference
                for segment in familiar.response.projection.segments
                for target in segment.sources
            }
            assert relational.reference not in {
                target.reference
                for segment in familiar.response.projection.segments
                for target in segment.sources
            }
            await adopt(
                SocialRelationBody(
                    from_entity=actor.reference,
                    to_entity=friend.reference,
                    relation="known",
                    dimension="familiarity",
                    degree=0.8,
                    judgement="sufficient",
                    epistemic=EpistemicStance(confidence=0.9),
                ),
                "actual-relationship",
            )
            related = await app.contexts.prepare(
                ctx,
                ContextPrepare(
                    operation_id="related",
                    session_id="related",
                    consumer_id="default",
                    instance_id="test",
                    usage=ContextUsage(
                        purpose="reply",
                        view="subject",
                        situations=("familiar",),
                        audience_entities=(friend.reference,),
                    ),
                ),
            )
            assert relational.reference in {
                target.reference
                for segment in related.response.projection.segments
                for target in segment.sources
            }
            worker = await app.contexts.prepare(
                ctx,
                ContextPrepare(
                    operation_id="worker",
                    session_id="worker",
                    consumer_id="default",
                    instance_id="test",
                    usage=ContextUsage(purpose="work", view="execution"),
                ),
            )
            assert worker.response.projection.segments == ()
            denied = await app.contexts.prepare(
                ctx.model_copy(update={"allowed_source_ids": ()}),
                ContextPrepare(
                    operation_id="denied",
                    session_id="denied",
                    consumer_id="default",
                    instance_id="test",
                    usage=ContextUsage(purpose="reply", view="execution"),
                ),
            )
            assert denied.response.projection.segments == ()
            assert "fictional repairer" not in denied.model_dump_json()

    asyncio.run(scenario())


def test_maintain_requires_full_observation_coverage_and_reports_a_violation(tmp_path):
    async def scenario():
        async with application(Settings(instance=tmp_path / "instance")) as app:
            ctx = CallContext(
                caller_id="owner",
                namespace_id="ns",
                subject_id="subject",
                credential_source_ids=None,
            )
            await app.identity.create_namespace(ctx, "Namespace", "ns")
            await app.identity.create_subject(
                ctx,
                "Subject",
                "subject",
                subject_id="subject",
                capabilities=("memory", "motivation"),
            )

            async def fact(value, start, end):
                result = await app.facts.change(
                    ctx,
                    FactChange(
                        action="assert",
                        claim=FactClaimInput(
                            scope=ChatFactScopeInput(conversation_ref="status"),
                            fact_key="system.available",
                            value_text=value,
                            cardinality="set",
                            stability="temporal",
                        ),
                        validity=TimeRange(start=start, end=end),
                        refresh_profile=False,
                    ),
                )
                return result.record.reference

            first = await fact("true", 0, 40)
            second = await fact("true", 60, 100)
            condition = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    body=DesireBody(
                        description="Maintain availability",
                        condition=DesiredCondition(
                            predicate="system.available", mode="maintain", evidence=(first, second)
                        ),
                        strength=0.5,
                    ),
                    evidence=(EvidenceLink(reference=first), EvidenceLink(reference=second)),
                ),
            )
            request = AssessMotivation(
                reference=condition.reference,
                period=TimeRange(start=0, end=100),
                at=TimePoint(coordinate=100),
            )
            incomplete = await app.motivation.assess_motivation(ctx, request)
            assert incomplete.satisfaction == "unknown"
            assert incomplete.gaps == (TimeRange(start=40, end=60),)
            middle = await fact("true", 40, 60)
            complete = await app.motivation.assess_motivation(
                ctx, request.model_copy(update={"evidence": (middle,)})
            )
            assert complete.satisfaction == "true" and complete.gaps == ()
            violation = await fact("false", 50, 51)
            failed = await app.motivation.assess_motivation(
                ctx, request.model_copy(update={"evidence": (middle, violation)})
            )
            assert failed.satisfaction == "false" and violation in failed.evidence

            await app.facts.change(
                ctx, FactChange(action="retract", reference=first, refresh_profile=False)
            )
            retracted = await app.motivation.assess_motivation(
                ctx, request.model_copy(update={"evidence": (middle,)})
            )
            assert retracted.satisfaction == "unknown" and first not in retracted.evidence
            assert retracted.gaps == (TimeRange(start=0, end=40),)

            current = ctx.clock.coordinate
            future = await fact("true", current + 1000, current + 2000)
            target = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    body=DesireBody(
                        description="Achieve availability",
                        condition=DesiredCondition(
                            predicate="system.available", mode="achieve", evidence=(future,)
                        ),
                        strength=0.5,
                    ),
                    evidence=(EvidenceLink(reference=future),),
                ),
            )
            not_yet = await app.motivation.assess_motivation(
                ctx,
                AssessMotivation(
                    reference=target.reference,
                    at=ctx.clock,
                    period=TimeRange(start=current, end=current + 1),
                ),
            )
            assert not_yet.satisfaction == "unknown" and not_yet.evidence == ()

            before = await fact("true", 0, 10)
            after = await fact("false", 10, 20)
            cessation = await app.cognition.adopt(
                ctx,
                AdoptCognition(
                    body=DesireBody(
                        description="Cease availability",
                        condition=DesiredCondition(
                            predicate="system.available", mode="cease", evidence=(before, after)
                        ),
                        strength=0.5,
                    ),
                    evidence=(EvidenceLink(reference=before), EvidenceLink(reference=after)),
                ),
            )
            changed = await app.motivation.assess_motivation(
                ctx,
                AssessMotivation(
                    reference=cessation.reference,
                    at=TimePoint(coordinate=20),
                    period=TimeRange(start=0, end=20),
                ),
            )
            assert changed.satisfaction == "true" and changed.transition_window == TimeRange(
                start=10, end=11
            )
            web = create_app(app.settings)
            web.state.application = app
            async with NousClient("http://test", credential_file=app.settings.token_file) as api:
                await api.transport.http.aclose()
                api.transport.http = httpx.AsyncClient(
                    transport=httpx.ASGITransport(app=web), base_url="http://test"
                )
                save = AssessMotivation(
                    operation_id="save-observed-transition",
                    reference=cessation.reference,
                    at=TimePoint(coordinate=20),
                    period=TimeRange(start=0, end=20),
                    save=True,
                )
                saved = await api.assess_motivation("ns", "subject", save)
                assert saved.assessment_id and saved.satisfaction == "true"
                assert await api.assess_motivation("ns", "subject", save) == saved
                second_saved = await api.assess_motivation(
                    "ns",
                    "subject",
                    save.model_copy(update={"operation_id": "save-transition-again"}),
                )
                history = await api.motivation_assessments(
                    "ns",
                    "subject",
                    MotivationAssessmentsQuery(reference=cessation.reference, limit=1),
                )
                assert history.items[0].id == second_saved.assessment_id and history.next_cursor
                next_page = await api.motivation_assessments(
                    "ns",
                    "subject",
                    MotivationAssessmentsQuery(
                        reference=cessation.reference, limit=1, cursor=history.next_cursor
                    ),
                )
                assert (
                    next_page.items[0].id == saved.assessment_id and next_page.next_cursor is None
                )
                original = await api.motivation_assessment("ns", "subject", saved.assessment_id)
                assert original.condition == cessation.body.condition
                assert original.assessment.evaluated_at == TimePoint(coordinate=20)
                prepared = await app.contexts.prepare(
                    ctx,
                    ContextPrepare(
                        session_id="saved-assessment",
                        consumer_id="default",
                        instance_id="test",
                        usage=ContextUsage(purpose="reply", intent="Cease availability"),
                    ),
                )
                segments = prepared.response.projection.segments
                assert any(
                    segment.provenance.get("motivation_assessment_id") == second_saved.assessment_id
                    and '"transition_window"' in segment.text
                    for segment in segments
                )
                revised = await app.cognition.adopt(
                    ctx,
                    AdoptCognition(
                        id=cessation.reference.id,
                        expected_revision=cessation.reference.revision,
                        body=cessation.body.model_copy(
                            update={
                                "condition": cessation.body.condition.model_copy(
                                    update={"mode": "avoid"}
                                )
                            }
                        ),
                        evidence=cessation.evidence,
                    ),
                )
                prepared = await app.contexts.prepare(
                    ctx,
                    ContextPrepare(
                        session_id="revised-condition",
                        consumer_id="default",
                        instance_id="test",
                        usage=ContextUsage(purpose="reply", intent="Cease availability"),
                    ),
                )
                assert all(
                    "motivation_assessment_id" not in segment.provenance
                    for segment in prepared.response.projection.segments
                )
                assert (
                    await api.motivation_assessment("ns", "subject", saved.assessment_id)
                ).condition.mode == "cease"
                namespace_ctx = ctx.model_copy(update={"subject_id": None})
                backup = await app.namespace_backups.create(namespace_ctx)
                restored = await app.namespace_backups.restore(
                    namespace_ctx.model_copy(update={"namespace_id": "copy"}),
                    RestoreNamespaceBackup(backup_id=backup.backup_id),
                )
                copied_ctx = ctx.model_copy(
                    update={"namespace_id": "copy", "subject_id": restored.mapping["subject"]}
                )
                copied = await app.motivation.motivation_assessment(
                    copied_ctx, restored.mapping[saved.assessment_id]
                )
                assert copied.assessment.reference.id == restored.mapping[cessation.reference.id]
                assert copied.assessment.reference.revision == cessation.reference.revision
                assert copied.condition.evidence[0].id in restored.mapping.values()
                retired = await app.sources.trash(
                    ctx, SourceCommand(references=(revised.reference,), reason="Remove evaluated condition")
                )
                await app.sources.purge(
                    ctx, SourceCommand(references=retired.references, force=True, reason="Erase saved evaluations with condition")
                )
                with pytest.raises(NousError):
                    await api.motivation_assessment("ns", "subject", saved.assessment_id)
                assert (
                    await api.motivation_assessments("ns", "subject", MotivationAssessmentsQuery())
                ).items == ()
                assert await app.models.ledger.list(ctx) == []

    asyncio.run(scenario())
