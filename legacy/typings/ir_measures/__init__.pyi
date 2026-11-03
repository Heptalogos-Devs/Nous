from collections.abc import Iterable, Mapping

class Measure:
    def __matmul__(self, cutoff: int) -> Measure: ...

P: Measure
R: Measure
nDCG: Measure

def calc_aggregate(
    measures: Iterable[Measure],
    qrels: Mapping[str, Mapping[str, int]],
    run: Mapping[str, Mapping[str, float]],
) -> dict[Measure, float]: ...
