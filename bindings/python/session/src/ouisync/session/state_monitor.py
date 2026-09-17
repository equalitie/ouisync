from dataclasses import dataclass


@dataclass(frozen=True, order=True)
class MonitorId:
    name: str
    disambiguator: int

    @staticmethod
    def parse(raw: str) -> "MonitorId":
        name, _, disambiguator = raw.rpartition(":")
        return MonitorId(name, int(disambiguator))

    def __str__(self) -> str:
        return f"{self.name}:{self.disambiguator}"
