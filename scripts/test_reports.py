"""Reports are derived only after strict acceptance, never used as its oracle."""

from dataclasses import asdict, dataclass, replace
import json
from pathlib import Path
import re
import xml.etree.ElementTree as ET


@dataclass(frozen=True)
class Result:
    scenario: str
    name: str
    status: str
    seconds: float | None
    log: str
    diagnostic: str = ""
    iteration: int = 0


def xml_text(value):
    return "".join(
        character if character in "\t\n\r" or 0x20 <= ord(character) <= 0xd7ff
        or 0xe000 <= ord(character) <= 0xfffd or 0x10000 <= ord(character) <= 0x10ffff
        else f"\\x{ord(character):02x}" for character in value
    )


def accepted_results(scenario, output, seconds, log, iteration=0):
    results = []
    pending = []
    for line in output.splitlines():
        match = re.fullmatch(r"(ok|not ok) [1-9][0-9]* ([a-z0-9_.]+)", line)
        if match:
            results.append(Result(
                scenario, match[2], "passed" if match[1] == "ok" else "failed",
                None, str(log), "\n".join(pending), iteration,
            ))
            pending = []
        elif line.startswith("# stage=") and results and results[-1].status == "failed":
            results[-1] = replace(results[-1], diagnostic=results[-1].diagnostic + "\n" + line)
        elif line.startswith("# "):
            pending.append(line)
    if not results:
        return [Result(scenario, scenario, "passed", seconds, str(log), iteration=iteration)]
    # Only the scenario's wall time is known; don't invent per-test timings.
    return results


def write_reports(results, json_path=None, junit_path=None):
    if json_path:
        path = Path(json_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({
            "schema": 1, "results": [asdict(result) for result in results],
        }, indent=2) + "\n", encoding="utf-8")
    if junit_path:
        suites = ET.Element("testsuites")
        for scenario, iteration in dict.fromkeys((result.scenario, result.iteration) for result in results):
            cases = [result for result in results if (result.scenario, result.iteration) == (scenario, iteration)]
            suite = ET.SubElement(suites, "testsuite", {
                "name": f"{scenario}.iteration-{iteration}", "tests": str(len(cases)),
                "failures": str(sum(result.status == "failed" for result in cases)),
                "errors": str(sum(result.status == "error" for result in cases)),
                "skipped": "0",
            })
            for result in cases:
                attributes = {"name": result.name, "classname": f"{scenario}.iteration-{iteration}"}
                if result.seconds is not None:
                    attributes["time"] = f"{result.seconds:.6f}"
                case = ET.SubElement(suite, "testcase", attributes)
                if result.status in ("failed", "error"):
                    ET.SubElement(case, "failure" if result.status == "failed" else "error").text = xml_text(result.diagnostic)
                ET.SubElement(case, "system-out").text = xml_text(f"log: {result.log}\n{result.diagnostic}")
        path = Path(junit_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        ET.indent(suites)
        ET.ElementTree(suites).write(path, encoding="utf-8", xml_declaration=True)
