"""Structural identity changes preserve factual attribution and current graph consumers."""

import asyncio

from nous_sdk.contracts import (
    AdoptCognition,
    AssociationBody,
    ConceptBody,
    ConceptPartition,
    EntityBody,
    EntityPartition,
    EvidenceLink,
    ImportDocument,
    MemoryBody,
    MergeConcepts,
    MergeEntities,
    RecordsQuery,
    ReferencePartition,
    SplitConcept,
    SplitEntity,
)
from nous.platform.call_context import CallContext
from nous_sdk.contracts.cognition import PlatformIdentity
from nous_sdk.contracts.fact_body import PersonFactScope
from nous_sdk.contracts.facts import FactChange, FactClaimInput, FactQuery, PersonFactScopeInput

from nous.application import Application
from nous.platform.configuration import Settings


def test_merge_split_migrates_graph_and_fact_scopes_without_rewriting_history(tmp_path):
    async def run():
        app = Application(Settings(instance=tmp_path / "instance"))
        await app.start()
        ctx = CallContext(
            caller_id="owner", namespace_id="n", subject_id="s", credential_source_ids=None
        )
        try:
            await app.identity.create_namespace(ctx, "N", "namespace")
            await app.identity.create_subject(
                ctx, "S", "subject", subject_id="s", capabilities=("memory", "social")
            )
            async with app.authority.write(ctx) as tx:
                original = await app.material.admit_text_in(
                    tx, ctx, ImportDocument(text="Identity correction report")
                )
                evidence = (EvidenceLink(observation_id=original.event["id"]),)
                memory = await app.cognition.adopt_in(
                    tx, ctx, AdoptCognition(body=MemoryBody(text="First source"), evidence=evidence)
                )
                other = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(body=MemoryBody(text="Second source"), evidence=evidence),
                )
                alice = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=EntityBody(
                            label="Alice",
                            platform_identities=(
                                PlatformIdentity(
                                    platform="host", account_id="alice", evidence=evidence
                                ),
                            ),
                        ),
                        evidence=evidence,
                    ),
                )
                alias = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=EntityBody(
                            label="Alicia",
                            platform_identities=(
                                PlatformIdentity(
                                    platform="host", account_id="alicia", evidence=evidence
                                ),
                            ),
                        ),
                        evidence=evidence,
                    ),
                )
                one = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=ConceptBody(label="Company", members=(memory.reference,)),
                        evidence=evidence,
                    ),
                )
                two = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=ConceptBody(label="Organization", members=(other.reference,)),
                        evidence=evidence,
                    ),
                )
                relation = await app.cognition.adopt_in(
                    tx,
                    ctx,
                    AdoptCognition(
                        body=AssociationBody(
                            from_ref=two.reference,
                            to_ref=alias.reference,
                            relation="employs",
                            basis_class="source_evidence",
                        ),
                        evidence=evidence,
                    ),
                )
                {'memory': memory.reference.model_dump(mode='json')}
            left = (
                await app.facts.change(
                    ctx,
                    FactChange(
                        action="assert",
                        refresh_profile=False,
                        claim=FactClaimInput(
                            scope=PersonFactScopeInput(person=alice.reference),
                            fact_key="team",
                            value_text="Alpha",
                            cardinality="single",
                        ),
                    ),
                )
            ).record
            right = (
                await app.facts.change(
                    ctx,
                    FactChange(
                        action="assert",
                        refresh_profile=False,
                        claim=FactClaimInput(
                            scope=PersonFactScopeInput(person=alias.reference),
                            fact_key="team",
                            value_text="Beta",
                            cardinality="single",
                        ),
                    ),
                )
            ).record
            concepts = await app.evolution.merge_concepts(
                ctx,
                MergeConcepts(
                    target=one.reference, sources=(two.reference,), reason="Same organization"
                ),
            )
            merged_concept = concepts[0]
            current_relation = next(
                record for record in concepts if record.reference.id == relation.reference.id
            )
            assert current_relation.body.from_ref == merged_concept.reference
            assert (await app.cognition.get(ctx, relation.reference)).body.from_ref == two.reference
            merged = await app.entities.merge_entities(
                ctx,
                MergeEntities(
                    target=alice.reference, sources=(alias.reference,), reason="Same person"
                ),
            )
            merged_person = merged[0]
            assert (
                await app.social.resolve_identity(ctx, "host", "alicia")
            ).reference == merged_person.reference
            claims = await app.facts.query(
                ctx,
                FactQuery(
                    scope=PersonFactScope(person=merged_person.reference), statuses=("conflicted",)
                ),
            )
            assert {record.body.value_text for record in claims} == {"Alpha", "Beta"}
            assert (await app.cognition.get(ctx, left.reference)).body.status == "active"
            migrated = next(record for record in claims if record.body.value_text == "Beta")
            assert (
                migrated.reference.id != right.reference.id
                and right.reference in migrated.body.identity_lineage
            )
            assert (
                await app.cognition.get(ctx, right.reference)
            ).body.scope.person == alias.reference
            current_relation = next(
                record for record in merged if record.reference.id == relation.reference.id
            )
            assert current_relation.body.to_ref == merged_person.reference
            split = await app.entities.split_entity(
                ctx,
                SplitEntity(
                    source=merged_person.reference,
                    partitions=tuple(
                        EntityPartition(
                            body=EntityBody(label=value, platform_identities=(platform,)),
                            claims=(record.reference,),
                        )
                        for value, record, platform in zip(
                            ("Alice", "Alicia"),
                            claims,
                            merged_person.body.platform_identities,
                            strict=True,
                        )
                    ),
                    consumers=(
                        ReferencePartition(consumer=current_relation.reference, partition=0),
                    ),
                    reason="Distinct people",
                ),
            )
            people = split[:2]
            for person in people:
                account = person.body.platform_identities[0]
                assert (
                    await app.social.resolve_identity(ctx, account.platform, account.account_id)
                ).reference == person.reference
            for person in people:
                current = await app.facts.query(
                    ctx, FactQuery(scope=PersonFactScope(person=person.reference))
                )
                assert len(current) == 1 and current[0].body.status == "active"
            current_relation = next(
                record for record in split if record.reference.id == relation.reference.id
            )
            divided = await app.evolution.split_concept(
                ctx,
                SplitConcept(
                    source=merged_concept.reference,
                    partitions=(
                        ConceptPartition(label="Company", members=(memory.reference,)),
                        ConceptPartition(label="Organization", members=(other.reference,)),
                    ),
                    consumers=(
                        ReferencePartition(consumer=current_relation.reference, partition=1),
                    ),
                    reason="Separate meanings",
                ),
            )
            current_relation = next(
                record for record in divided if record.reference.id == relation.reference.id
            )
            assert current_relation.body.from_ref == divided[1].reference
            page = await app.cognition.records(ctx, RecordsQuery(kinds=("entity",)))
            assert {entry.record.reference for entry in page.items} == {
                record.reference for record in people
            }
            # Bundle closure also validates cross-scope claim lineage and exact prior attribution.
            await app.bundles.export(ctx)
        finally:
            await app.close()

    asyncio.run(run())
