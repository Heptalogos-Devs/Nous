"""Selection coverage preserves the exact parts of source material."""

from itertools import combinations, product

from nous.content.contracts import ArtifactRef, ContentSelection, SourceSelection
from nous.content.selections import subtract_selection, union_selections


def test_subtraction_matches_byte_set_difference() -> None:
    artifact = ArtifactRef(instance_id="instance", id="body", identifier="art-body")

    def selected(start: int, end: int) -> ContentSelection:
        return ContentSelection(
            artifact=artifact, selection=SourceSelection(start_byte=start, end_byte=end)
        )

    spans = tuple(combinations(range(6), 2))
    for (start, end), (left, right), (other_left, other_right) in product(spans, repeat=3):
        actual = subtract_selection(
            selected(start, end), (selected(left, right), selected(other_left, other_right))
        )
        expected = (
            set(range(start, end)) - set(range(left, right)) - set(range(other_left, other_right))
        )
        pieces = [
            point
            for item in actual
            for point in range(item.selection.start_byte or 0, item.selection.end_byte or 0)
        ]
        assert set(pieces) == expected
        assert len(pieces) == len(expected)
        joined = union_selections(
            (selected(start, end), selected(left, right), selected(other_left, other_right))
        )
        union = (
            set(range(start, end)) | set(range(left, right)) | set(range(other_left, other_right))
        )
        merged = [
            point
            for item in joined
            for point in range(item.selection.start_byte or 0, item.selection.end_byte or 0)
        ]
        assert set(merged) == union
        assert len(merged) == len(union)


def test_subtraction_preserves_other_coordinate_families() -> None:
    artifact = ArtifactRef(instance_id="instance", id="body", identifier="art-body")

    def selected(**values: object) -> ContentSelection:
        return ContentSelection(artifact=artifact, selection=SourceSelection.model_validate(values))

    assert subtract_selection(selected(pages=(1, 2, 3)), (selected(pages=(2,)),)) == (
        selected(pages=(1, 3)),
    )
    assert subtract_selection(
        selected(start_line=2, end_line=10),
        (selected(start_line=3, end_line=7), selected(start_line=8, end_line=9)),
    ) == (selected(start_line=2, end_line=2), selected(start_line=10, end_line=10))
    assert subtract_selection(
        selected(start_ms=0, end_ms=20), (selected(start_ms=5, end_ms=15),)
    ) == (selected(start_ms=0, end_ms=5), selected(start_ms=15, end_ms=20))
    region = selected(frame=2, crop=(0, 0, 10, 10))
    assert subtract_selection(region, (selected(frame=2, crop=(0, 0, 5, 5)),)) == (region,)
    assert union_selections((selected(start_byte=2, end_byte=4), selected())) == (selected(),)
