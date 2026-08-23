"""Persistent, operator-editable controlled vocabulary for Assistant tagging."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from sqlalchemy import CursorResult, update
from sqlalchemy.orm import Session

from app.assistant.tags import normalize_manual_tag
from app.models.assistant_tag_vocabulary import AssistantTagVocabulary
from app.models.base import utcnow

TAG_VOCABULARY_KEY = "library"
TAG_VOCABULARY_SCHEMA: Literal["assistant-tag-vocabulary/v1"] = (
    "assistant-tag-vocabulary/v1"
)
TAG_VOCABULARY_SEED_VERSION = 4
MAX_VOCABULARY_TAGS = 200

_DEFAULT_CONTEXT_CUES: dict[str, tuple[str, ...]] = {
    # Terrain and settings: indirect but high-signal phrases that often appear in
    # soundtrack titles, albums, and library folders.
    "setting.medieval": ("minstrel", "troubadour", "bardic", "feudal court", "knightly"),
    "setting.tavern": ("drinking song", "common room", "hearthside", "ale and song"),
    "setting.dungeon": ("underground delve", "dungeon crawl", "dungeon depths", "hostile delve"),
    "setting.castle": ("crown guard", "royal procession", "royal brass", "royal palace"),
    "setting.village": ("harvest village", "village fair", "rural folk"),
    "setting.town": ("town square", "town streets", "small town"),
    "setting.farm": ("farmstead", "harvest field", "rural fields"),
    "setting.forest": ("ancient pines", "forest path", "ranger patrol"),
    "setting.wilderness": ("open wilderness", "untamed frontier", "lonely wilds"),
    "setting.hills": ("rolling hills", "hill country"),
    "setting.plains": ("open plains", "grasslands", "prairie crossing"),
    "setting.temple": ("temple vigil", "sacred mysteries", "devotional choral"),
    "setting.ruins": ("fallen city", "ruined kingdom", "ancient remains"),
    "setting.seafaring": ("black sails", "broken masts", "sea battle", "ocean crossing"),
    "setting.city": ("city streets", "city watch", "urban chase"),
    "setting.market": ("market day", "merchant stalls", "trade quarter"),
    "setting.road": ("open road", "quiet roads", "wayfarer song"),
    "setting.mountains": ("mountain pass", "high peaks", "summit crossing"),
    "setting.desert": ("desert caravan", "desert crossing", "sand crossing"),
    "setting.swamp": ("murky fen", "swamp trail", "marshland crossing"),
    "setting.jungle": ("jungle expedition", "jungle ruins", "jungle trek"),
    "setting.cave": ("underground cavern", "cave exploration"),
    "setting.coast": ("shattered coast", "coastal village", "harbor cliffs"),
    "setting.ocean": ("open ocean", "deep water", "ocean waves"),
    "setting.lake": ("lakeside", "still lake", "lake shore"),
    "setting.river": ("river journey", "river crossing", "riverside camp"),
    "setting.island": ("island shore", "tropical isle", "remote island"),
    "setting.arctic": ("icebound", "frozen expedition", "glacial crossing"),
    "setting.tundra": ("frozen heath", "tundra crossing"),
    "setting.underwater": ("sunken city", "undersea ruins", "ocean depths"),
    "setting.sky": ("airship", "sky voyage", "cloud kingdom"),
    "setting.camp": ("campfire", "night camp", "roadside camp"),
    "setting.volcanic": ("lava flows", "volcanic crater", "burning mountain"),
    "setting.canyon": ("canyon passage", "deep ravine", "red cliffs"),
    "setting.battlefield": ("aftermath of battle", "fallen soldiers", "war camp"),
    "setting.graveyard": ("tombstones", "haunted cemetery", "graveyard vigil"),
    "setting.sewer": ("city sewers", "underground drains", "sewer tunnels"),
    "setting.mine": ("abandoned mine", "mine shafts", "mining tunnels"),
    "setting.prison": ("prison escape", "captive cell", "dungeon cell"),
    "setting.arena": ("gladiator", "arena battle", "tournament fight"),
    "setting.library": ("forbidden archive", "ancient library", "scholar stacks"),
    "setting.workshop": ("alchemist laboratory", "blacksmith forge", "tinkering shop"),
    "setting.court": ("court intrigue", "royal audience", "royal procession"),
    "setting.monastery": ("monastic chant", "monastery vigil", "monks"),
    "setting.fey-realm": ("pixie", "fairy court", "enchanted forest"),
    "setting.shadow-realm": ("umbral", "shadow plane", "land of shadow"),
    "setting.celestial-realm": ("angelic", "celestial choir", "radiant heavens"),
    "setting.infernal-realm": ("demonic", "devil", "hellfire", "abyssal gate"),
    "setting.astral-realm": ("cosmic void", "starlit void", "astral voyage"),
    "setting.elemental-realm": (
        "primordial plane",
        "elemental chaos",
        "plane of fire",
        "plane of water",
    ),
    # Scenes: concrete actions and event phrases. These may overlap with setting
    # and mood cues but never become cleanup aliases.
    "scene.dancing": ("dance floor", "courtly dance", "folk dance"),
    "scene.feast": ("harvest supper", "tavern meal"),
    "scene.travel": (
        "crossing",
        "journeys",
        "wayfarer",
        "passage",
        "road song",
    ),
    "scene.exploration": ("delving", "scouting", "mapping expedition"),
    "scene.combat": ("swords clash", "war drums", "battlefield fight", "fighting pit"),
    "scene.stealth": ("hidden approach", "silent passage", "moving in shadows"),
    "scene.investigation": ("mystery clues", "detective mystery", "searching for clues"),
    "scene.rest": ("campfire", "quiet night", "gentle sleep"),
    "scene.chase": ("fleeing", "running pursuit", "frantic race"),
    "scene.escape": ("fleeing captivity", "prison break", "desperate retreat"),
    "scene.ambush": ("hidden attackers", "attack from shadows"),
    "scene.siege": ("city under attack", "castle assault", "siege engines"),
    "scene.boss-battle": ("final battle", "climactic fight", "final duel"),
    "scene.ritual": ("incantation", "altar rite", "spellcasting", "dark ritual"),
    "scene.ceremony": ("royal procession", "temple rite", "formal observance"),
    "scene.worship": ("hymn", "sacred chant", "temple prayer"),
    "scene.negotiation": ("treaty talks", "diplomatic meeting", "peace talks"),
    "scene.intrigue": ("court secrets", "spies", "political plot"),
    "scene.conversation": ("fireside talk", "tavern talk", "quiet dialogue"),
    "scene.planning": ("war council", "tactical briefing", "council meeting"),
    "scene.preparation": (
        "before battle",
        "before the tournament",
        "preparing for war",
    ),
    "scene.shopping": ("market day", "merchant stalls", "browsing wares"),
    "scene.crafting": ("workshop", "forge work", "alchemy"),
    "scene.puzzle": ("ancient mechanism", "puzzle chamber", "brain teaser"),
    "scene.discovery": ("hidden chamber", "lost city found", "secret found"),
    "scene.survival": ("stranded", "harsh wilderness", "against the elements"),
    "scene.hunting": ("stalk prey", "hunter patrol", "tracking beast"),
    "scene.sailing": ("ship voyage", "ocean crossing", "black sails"),
    "scene.flying": ("airship", "sky voyage", "dragon flight"),
    "scene.courtship": ("lovers", "love song", "romantic meeting"),
    "scene.reunion": ("return home", "friends reunited"),
    "scene.farewell": ("last goodbye", "leaving home"),
    "scene.betrayal": ("poisoned trust", "traitor revealed", "backstab"),
    "scene.revelation": ("prophecy revealed", "hidden truth", "secret unveiled"),
    "scene.victory": ("victory fanfare", "battle won", "hard won success"),
    "scene.defeat": ("fallen heroes", "battle lost"),
    "scene.mourning": ("requiem", "lament", "memorial"),
    "scene.festival": ("village fair", "harvest festival", "holiday dance"),
    "scene.march": ("war drums", "crown guard", "military procession"),
    "scene.rescue": ("saving captive", "rescue mission", "race to save"),
    "scene.training": ("sparring", "practice yard", "combat drills"),
    "scene.storytelling": (
        "bard tale",
        "campfire story",
        "gentle tales",
        "ancient legend",
    ),
    # Moods: cross-axis emotional evidence. Prefer explicit event phrases over
    # generic correlations so fast models do not receive a wall of weak hints.
    "mood.festive": ("dance", "dances", "dancing", "jig", "feast", "banquet", "festival"),
    "mood.heroic": (
        "crown guard",
        "royal guard",
        "royal procession",
        "heroic march",
        "rescue mission",
        "victory fanfare",
        "brave stand",
    ),
    "mood.mysterious": (
        "riddle",
        "hidden secret",
        "forbidden archive",
        "unknown chamber",
        "whispers",
    ),
    "mood.tense": ("chase", "ambush", "siege", "pursuit", "gathering storm", "danger"),
    "mood.dark": ("shadow realm", "infernal realm", "demonic ritual", "abyssal gate"),
    "mood.calm": ("quiet", "lullaby", "gentle", "stillness", "rest", "campfire", "quiet night"),
    "mood.eerie": (
        "whisper",
        "whispers",
        "crypt",
        "catacomb",
        "catacombs",
        "haunted",
        "tombstones",
        "graveyard",
    ),
    "mood.melancholy": ("mourning", "funeral", "farewell", "requiem", "lament", "battle lost"),
    "mood.romantic": ("courtship", "lovers", "love song", "wedding"),
    "mood.joyful": ("reunion", "village fair", "festival", "laughter", "celebration"),
    "mood.hopeful": ("rescue", "homecoming", "dawn", "new beginning"),
    "mood.uplifting": ("victory", "homecoming", "rescue mission", "dawn"),
    "mood.playful": ("prank", "jester", "frolic", "jig"),
    "mood.whimsical": ("pixie", "fairy court", "feywild", "storybook"),
    "mood.wondrous": (
        "first sight",
        "discovery",
        "lost city found",
        "celestial realm",
        "astral realm",
    ),
    "mood.ethereal": ("celestial choir", "astral realm", "floating islands", "dreamscape"),
    "mood.majestic": (
        "royal procession",
        "throne room",
        "coronation",
        "celestial realm",
        "mountain summit",
    ),
    "mood.solemn": ("vigil", "requiem", "funeral", "coronation", "memorial"),
    "mood.sacred": ("worship", "prayer", "hymn", "temple vigil", "monastery", "celestial choir"),
    "mood.ominous": (
        "dark ritual",
        "approaching danger",
        "gathering storm",
        "prophecy of doom",
        "abyssal gate",
    ),
    "mood.anxious": ("hiding", "escape", "pursuit", "ambush", "ticking clock"),
    "mood.fearful": ("terror", "horror", "haunted", "monster attack", "fleeing"),
    "mood.aggressive": ("combat", "boss battle", "fighting pit", "swords clash", "war drums"),
    "mood.desperate": ("last stand", "prison escape", "stranded", "survival", "desperate retreat"),
    "mood.determined": ("training", "preparation", "march", "practice yard", "drills"),
    "mood.defiant": ("rebellion", "resistance", "uprising", "last stand"),
    "mood.adventurous": (
        "expedition",
        "quest",
        "journey",
        "exploration",
        "sky voyage",
        "ocean crossing",
    ),
    "mood.curious": ("investigation", "puzzle", "riddle", "hidden chamber", "discovery"),
    "mood.dreamy": ("lullaby", "sleep", "moonlight", "starlit", "dreamscape"),
    "mood.nostalgic": ("homecoming", "old memories", "childhood", "return home"),
    "mood.lonely": ("abandoned", "empty road", "solitary journey", "alone", "frozen heath"),
    "mood.bittersweet": ("farewell", "last goodbye", "parting"),
    "mood.warm": ("hearth", "campfire", "homecoming", "cozy inn", "fireside"),
    "mood.cold": ("glacier", "frozen wastes", "tundra", "icebound", "frozen heath"),
    "mood.chaotic": ("collapsing", "battle", "chase", "storm", "elemental chaos"),
    "mood.urgent": ("chase", "escape", "rescue", "race to save", "ticking clock"),
    "mood.triumphant": ("victory", "battle won", "coronation", "victory fanfare"),
    "mood.humorous": ("jester", "prank", "comedy", "comic relief", "absurd"),
    "mood.magical": ("spell", "spellbook", "sorcery", "ritual", "feywild", "enchanted forest"),
}


class _StrictVocabularyModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class TagVocabularyEntry(_StrictVocabularyModel):
    id: str = Field(pattern=r"^[a-z0-9][a-z0-9._-]{1,63}$")
    name: str = Field(min_length=1, max_length=64)
    description: str = Field(min_length=2, max_length=300)
    aliases: list[str] = Field(default_factory=list, max_length=24)
    context_cues: list[str] = Field(default_factory=list, max_length=32)

    @field_validator("name")
    @classmethod
    def normalize_name(cls, value: str) -> str:
        return normalize_manual_tag(value)

    @field_validator("description")
    @classmethod
    def normalize_description(cls, value: str) -> str:
        normalized = " ".join(value.split())
        if len(normalized) < 2:
            raise ValueError("a tag description must contain at least two characters")
        return normalized

    @field_validator("aliases", "context_cues")
    @classmethod
    def normalize_terms(cls, value: list[str]) -> list[str]:
        return list(dict.fromkeys(normalize_manual_tag(term) for term in value))

    @model_validator(mode="after")
    def aliases_do_not_repeat_name(self) -> TagVocabularyEntry:
        if self.name in self.aliases:
            raise ValueError("a tag name cannot also be one of its aliases")
        return self


class TagVocabularyGroup(_StrictVocabularyModel):
    key: str = Field(pattern=r"^[a-z0-9][a-z0-9_-]{1,31}$")
    label: str = Field(min_length=1, max_length=64)
    description: str = Field(default="", max_length=300)
    tags: list[TagVocabularyEntry] = Field(default_factory=list, max_length=100)

    @field_validator("label")
    @classmethod
    def normalize_label(cls, value: str) -> str:
        normalized = " ".join(value.split())
        if not normalized:
            raise ValueError("a vocabulary group label cannot be blank")
        return normalized

    @field_validator("description")
    @classmethod
    def normalize_group_description(cls, value: str) -> str:
        return " ".join(value.split())


class TagVocabularyDocument(_StrictVocabularyModel):
    schema_version: Literal["assistant-tag-vocabulary/v1"] = TAG_VOCABULARY_SCHEMA
    groups: list[TagVocabularyGroup] = Field(min_length=1, max_length=20)

    @model_validator(mode="after")
    def unique_vocabulary(self) -> TagVocabularyDocument:
        group_keys = [group.key for group in self.groups]
        if len(group_keys) != len(set(group_keys)):
            raise ValueError("vocabulary group keys must be unique")

        entries = [tag for group in self.groups for tag in group.tags]
        if not entries:
            raise ValueError("the vocabulary must contain at least one tag")
        if len(entries) > MAX_VOCABULARY_TAGS:
            raise ValueError(
                f"the vocabulary cannot contain more than {MAX_VOCABULARY_TAGS} tags"
            )
        ids = [tag.id for tag in entries]
        names = [tag.name for tag in entries]
        if len(ids) != len(set(ids)):
            raise ValueError("vocabulary tag IDs must be unique")
        if len(names) != len(set(names)):
            raise ValueError("vocabulary tag names must be unique")

        canonical_names = set(names)
        alias_owners: dict[str, str] = {}
        for tag in entries:
            for alias in tag.aliases:
                if alias in canonical_names:
                    raise ValueError(
                        f"alias '{alias}' conflicts with a canonical tag name"
                    )
                owner = alias_owners.get(alias)
                if owner is not None:
                    raise ValueError(
                        f"alias '{alias}' belongs to both '{owner}' and '{tag.name}'"
                    )
                alias_owners[alias] = tag.name
        return self


@dataclass(frozen=True)
class TagVocabularySnapshot:
    document: TagVocabularyDocument
    revision: int
    fingerprint: str

    @property
    def entries(self) -> tuple[TagVocabularyEntry, ...]:
        return tuple(tag for group in self.document.groups for tag in group.tags)

    @property
    def ids(self) -> frozenset[str]:
        return frozenset(tag.id for tag in self.entries)

    @property
    def names(self) -> frozenset[str]:
        return frozenset(tag.name for tag in self.entries)

    @property
    def by_id(self) -> dict[str, TagVocabularyEntry]:
        return {tag.id: tag for tag in self.entries}

    @property
    def by_name(self) -> dict[str, TagVocabularyEntry]:
        return {tag.name: tag for tag in self.entries}

    @property
    def aliases(self) -> dict[str, TagVocabularyEntry]:
        return {alias: tag for tag in self.entries for alias in tag.aliases}

    @property
    def group_by_tag_id(self) -> dict[str, TagVocabularyGroup]:
        return {
            tag.id: group
            for group in self.document.groups
            for tag in group.tags
        }


class TagVocabularyConflictError(ValueError):
    pass


def _entry(
    group: str,
    name: str,
    description: str,
    *aliases: str,
    context_cues: tuple[str, ...] = (),
) -> TagVocabularyEntry:
    tag_id = f"{group}.{name.replace(' ', '-')}"
    return TagVocabularyEntry(
        id=tag_id,
        name=name,
        description=description,
        aliases=list(aliases),
        context_cues=list(dict.fromkeys((*_DEFAULT_CONTEXT_CUES.get(tag_id, ()), *context_cues))),
    )
def default_tag_vocabulary() -> TagVocabularyDocument:
    return TagVocabularyDocument(
        groups=[
            TagVocabularyGroup(
                key="setting",
                label="Terrain & setting",
                description=(
                    "The physical terrain, built location, plane, or culture the music evokes."
                ),
                tags=[
                    _entry(
                        "setting",
                        "medieval",
                        "Pre-modern European courtly, folk, or feudal atmosphere.",
                        "middle ages",
                    ),
                    _entry(
                        "setting",
                        "tavern",
                        "Inn, alehouse, common-room, or drinking-house setting.",
                        "inn",
                        "pub",
                        "alehouse",
                    ),
                    _entry(
                        "setting",
                        "dungeon",
                        "Enclosed underground danger, crypt, catacomb, or hostile delve.",
                        "crypt",
                        "catacomb",
                        "catacombs",
                    ),
                    _entry(
                        "setting",
                        "castle",
                        "Fortified keep, palace, citadel, battlement, or great hall.",
                        "keep",
                        "palace",
                        "citadel",
                    ),
                    _entry(
                        "setting",
                        "village",
                        "Small inhabited rural settlement or homestead community.",
                        "hamlet",
                    ),
                    _entry(
                        "setting",
                        "forest",
                        "Temperate woodland, grove, canopy, or tree-dense natural setting.",
                        "woodland",
                        "woods",
                        "grove",
                    ),
                    _entry(
                        "setting",
                        "wilderness",
                        "Remote uncultivated land beyond settlements and maintained roads.",
                        "wilds",
                        "untamed lands",
                    ),
                    _entry(
                        "setting",
                        "temple",
                        "Sacred place, shrine, chapel, or organized religious setting.",
                        "shrine",
                        "chapel",
                        "sanctuary",
                    ),
                    _entry(
                        "setting",
                        "ruins",
                        "Abandoned, broken, or ancient constructed remains.",
                        "ruin",
                        "ruined city",
                    ),
                    _entry(
                        "setting",
                        "seafaring",
                        "Ships, sailors, ports, naval life, or a voyage at sea.",
                        "ocean voyage",
                        "nautical",
                        "maritime",
                        "fleet",
                        "sea",
                        "sailor",
                        "sails",
                        "naval",
                    ),
                    _entry(
                        "setting",
                        "city",
                        "Dense urban streets, districts, crowds, or metropolitan life.",
                        "urban",
                        "metropolis",
                    ),
                    _entry(
                        "setting",
                        "town",
                        "Established settlement larger than a village but smaller than a city.",
                        "borough",
                    ),
                    _entry(
                        "setting",
                        "market",
                        "Bazaar, marketplace, merchant quarter, or busy trade setting.",
                        "bazaar",
                        "marketplace",
                        "merchant quarter",
                    ),
                    _entry(
                        "setting",
                        "farm",
                        "Farmland, cultivated fields, ranches, or pastoral homesteads.",
                        "farmland",
                        "fields",
                        "pastoral",
                    ),
                    _entry(
                        "setting",
                        "road",
                        "Road, trail, crossroads, or maintained route through the landscape.",
                        "trail",
                        "crossroads",
                        "highway",
                    ),
                    _entry(
                        "setting",
                        "mountains",
                        "High peaks, alpine slopes, cliffs, or a mountain pass.",
                        "mountain",
                        "alpine",
                        "peak",
                        "peaks",
                    ),
                    _entry(
                        "setting",
                        "hills",
                        "Rolling hills, highlands, moors, or elevated countryside.",
                        "hill",
                        "highlands",
                        "moorland",
                    ),
                    _entry(
                        "setting",
                        "plains",
                        "Open grassland, prairie, steppe, or broad treeless country.",
                        "plain",
                        "grassland",
                        "prairie",
                        "steppe",
                    ),
                    _entry(
                        "setting",
                        "desert",
                        "Arid sand, dunes, rocky wastes, or water-scarce badlands.",
                        "dunes",
                        "sand dunes",
                        "wasteland",
                        "badlands",
                    ),
                    _entry(
                        "setting",
                        "swamp",
                        "Wetland, marsh, bog, fen, or flooded lowland.",
                        "marsh",
                        "bog",
                        "wetland",
                        "fen",
                    ),
                    _entry(
                        "setting",
                        "jungle",
                        "Dense tropical forest, rainforest, or overgrown humid wilderness.",
                        "rainforest",
                        "tropical forest",
                    ),
                    _entry(
                        "setting",
                        "cave",
                        "Natural cavern, grotto, tunnel, or subterranean chamber.",
                        "cavern",
                        "caverns",
                        "grotto",
                    ),
                    _entry(
                        "setting",
                        "coast",
                        "Shoreline, beach, sea cliffs, or land beside open water.",
                        "coastal",
                        "shoreline",
                        "beach",
                        "sea cliffs",
                    ),
                    _entry(
                        "setting",
                        "ocean",
                        "Open sea, deep water, waves, or a vast marine expanse.",
                        "open sea",
                        "deep sea",
                    ),
                    _entry(
                        "setting",
                        "river",
                        "River, stream, creek, rapids, or waterside route.",
                        "riverside",
                        "stream",
                        "creek",
                        "rapids",
                    ),
                    _entry(
                        "setting",
                        "lake",
                        "Inland lake, pond, reservoir, or quiet lakeside setting.",
                        "lakeside",
                        "pond",
                    ),
                    _entry(
                        "setting",
                        "island",
                        "Island, isle, archipelago, or isolated land surrounded by water.",
                        "isle",
                        "archipelago",
                    ),
                    _entry(
                        "setting",
                        "arctic",
                        "Glacier, sea ice, polar wastes, or permanently frozen terrain.",
                        "glacier",
                        "frozen wastes",
                        "sea ice",
                    ),
                    _entry(
                        "setting",
                        "tundra",
                        "Cold treeless plain, frozen heath, or sparse northern terrain.",
                        "frozen plain",
                        "frozen heath",
                    ),
                    _entry(
                        "setting",
                        "volcanic",
                        "Volcano, lava field, magma chamber, ash land, or fiery geology.",
                        "volcano",
                        "lava",
                        "magma",
                        "ash land",
                    ),
                    _entry(
                        "setting",
                        "canyon",
                        "Deep gorge, ravine, chasm, or steep-sided valley.",
                        "gorge",
                        "ravine",
                        "chasm",
                    ),
                    _entry(
                        "setting",
                        "underwater",
                        "Submerged, sunken, or beneath-the-waves environment.",
                        "submerged",
                        "sunken",
                        "beneath the waves",
                    ),
                    _entry(
                        "setting",
                        "sky",
                        "Clouds, airborne travel, floating islands, or open upper air.",
                        "airborne",
                        "clouds",
                        "floating islands",
                    ),
                    _entry(
                        "setting",
                        "camp",
                        "Campsite, campfire, temporary encampment, or roadside bivouac.",
                        "campsite",
                        "campfire",
                        "encampment",
                    ),
                    _entry(
                        "setting",
                        "battlefield",
                        "Battleground, war-torn field, siege line, or aftermath of conflict.",
                        "battleground",
                        "warzone",
                    ),
                    _entry(
                        "setting",
                        "graveyard",
                        "Cemetery, burial ground, tomb field, or place of the dead.",
                        "cemetery",
                        "burial ground",
                    ),
                    _entry(
                        "setting",
                        "sewer",
                        "Sewers, drains, cisterns, or filthy tunnels beneath a settlement.",
                        "sewers",
                        "drainage tunnels",
                        "cistern",
                    ),
                    _entry(
                        "setting",
                        "mine",
                        "Working or abandoned mine, quarry, or excavated resource tunnel.",
                        "mines",
                        "quarry",
                    ),
                    _entry(
                        "setting",
                        "prison",
                        "Jail, gaol, cell block, dungeon cell, or place of confinement.",
                        "jail",
                        "gaol",
                        "cell block",
                    ),
                    _entry(
                        "setting",
                        "arena",
                        "Colosseum, fighting pit, tournament ground, or public contest venue.",
                        "colosseum",
                        "fighting pit",
                        "tournament ground",
                    ),
                    _entry(
                        "setting",
                        "library",
                        "Library, archive, scriptorium, or scholarly collection.",
                        "archive",
                        "archives",
                        "scriptorium",
                    ),
                    _entry(
                        "setting",
                        "workshop",
                        "Forge, smithy, laboratory, studio, or place of making.",
                        "forge",
                        "smithy",
                        "laboratory",
                    ),
                    _entry(
                        "setting",
                        "court",
                        "Royal court, throne room, noble household, or formal audience chamber.",
                        "royal court",
                        "throne room",
                    ),
                    _entry(
                        "setting",
                        "monastery",
                        "Monastery, abbey, cloister, or secluded religious community.",
                        "abbey",
                        "cloister",
                    ),
                    _entry(
                        "setting",
                        "fey realm",
                        "Enchanted faerie realm with mercurial natural magic.",
                        "feywild",
                        "faerie realm",
                    ),
                    _entry(
                        "setting",
                        "shadow realm",
                        "Plane of shadow, muted reflection, or supernatural gloom.",
                        "shadowfell",
                        "plane of shadow",
                    ),
                    _entry(
                        "setting",
                        "celestial realm",
                        "Heavenly, divine, radiant, or upper-planar domain.",
                        "heaven",
                        "divine realm",
                    ),
                    _entry(
                        "setting",
                        "infernal realm",
                        "Hellish, demonic, abyssal, or lower-planar domain.",
                        "hell",
                        "abyss",
                        "demonic realm",
                    ),
                    _entry(
                        "setting",
                        "astral realm",
                        "Astral plane, cosmic void, starscape, or space between worlds.",
                        "astral plane",
                        "cosmic void",
                        "starscape",
                    ),
                    _entry(
                        "setting",
                        "elemental realm",
                        "Plane dominated by elemental fire, water, air, or earth.",
                        "elemental plane",
                    ),
                ],
            ),
            TagVocabularyGroup(
                key="scene",
                label="Scene",
                description="What the players or characters are doing.",
                tags=[
                    _entry(
                        "scene",
                        "dancing",
                        "Rhythmic social, folk, courtly, or celebratory dance.",
                        "dance",
                        "dances",
                        "jig",
                        "reel",
                        "waltz",
                    ),
                    _entry(
                        "scene",
                        "feast",
                        "Banquet, communal meal, revel, or abundant celebration.",
                        "banquet",
                        "communal meal",
                    ),
                    _entry(
                        "scene",
                        "travel",
                        "A journey, road sequence, voyage, or movement between places.",
                        "journey",
                        "overland travel",
                        "voyage",
                        "caravan",
                    ),
                    _entry(
                        "scene",
                        "exploration",
                        "Surveying, delving, mapping, or cautiously entering the unknown.",
                        "expedition",
                        "adventuring",
                    ),
                    _entry(
                        "scene",
                        "combat",
                        "Active battle, confrontation, attack, or martial conflict.",
                        "battle",
                        "battles",
                        "fight",
                        "skirmish",
                        "war",
                    ),
                    _entry(
                        "scene",
                        "stealth",
                        "Sneaking, infiltration, hiding, or avoiding detection.",
                        "sneaking",
                        "infiltration",
                        "covert approach",
                    ),
                    _entry(
                        "scene",
                        "investigation",
                        "Searching for clues, deduction, inquiry, or detective work.",
                        "detective work",
                        "inquiry",
                        "clue search",
                    ),
                    _entry(
                        "scene",
                        "rest",
                        "Recovery, sleep, camp, respite, or a safe pause.",
                        "repose",
                        "quiet sleep",
                        "respite",
                        "sleep",
                        "lullaby",
                    ),
                    _entry("scene", "chase", "Fast pursuit, race, or running hunt.", "pursuit", "race"),
                    _entry("scene", "escape", "Breakout, retreat, evasion, or flight from danger.", "breakout", "retreat"),
                    _entry("scene", "ambush", "Surprise attack, hidden assault, or sudden trap.", "surprise attack"),
                    _entry("scene", "siege", "Assault or defense of a fortified position.", "castle siege", "siege battle"),
                    _entry("scene", "boss battle", "Climactic confrontation with a major singular threat.", "showdown", "final confrontation"),
                    _entry("scene", "ritual", "Summoning, invocation, magical rite, or formal occult act.", "summoning", "invocation"),
                    _entry("scene", "ceremony", "Formal coronation, wedding, investiture, or civic rite.", "coronation", "wedding ceremony"),
                    _entry("scene", "worship", "Prayer, devotion, religious service, or communion.", "prayer", "devotion"),
                    _entry("scene", "negotiation", "Diplomacy, bargaining, parley, or terms under discussion.", "diplomacy", "parley"),
                    _entry("scene", "intrigue", "Plotting, conspiracy, political maneuvering, or hidden agendas.", "plotting", "conspiracy"),
                    _entry("scene", "conversation", "Dialogue, social encounter, or character-focused exchange.", "dialogue", "social encounter"),
                    _entry("scene", "planning", "Strategy meeting, council, tactical discussion, or forming a plan.", "strategy", "council"),
                    _entry("scene", "preparation", "Gearing up, readying supplies, or preparing for action.", "gearing up", "readying"),
                    _entry("scene", "shopping", "Buying, selling, trading, or browsing wares.", "buying", "trade"),
                    _entry("scene", "crafting", "Smithing, brewing, building, enchanting, or making an item.", "smithing", "brewing"),
                    _entry("scene", "puzzle", "Riddle, mechanism, problem solving, or intellectual obstacle.", "riddle", "problem solving"),
                    _entry("scene", "discovery", "Finding something new, uncovering a place, or first revelation.", "uncovering", "first sight"),
                    _entry("scene", "survival", "Endurance, exposure, scarcity, or hardship against the environment.", "endurance", "hardship"),
                    _entry("scene", "hunting", "Tracking, stalking, or pursuing prey.", "hunt", "tracking prey"),
                    _entry("scene", "sailing", "Operating or travelling aboard a ship or boat.", "shipboard journey", "boat travel"),
                    _entry("scene", "flying", "Airborne journey, aerial maneuver, or travel through the sky.", "aerial journey", "air travel"),
                    _entry("scene", "courtship", "Romantic encounter, wooing, intimacy, or forming a bond.", "wooing", "romantic encounter"),
                    _entry("scene", "reunion", "Homecoming or meeting again after separation.", "homecoming"),
                    _entry("scene", "farewell", "Goodbye, departure, parting, or leave-taking.", "goodbye", "parting"),
                    _entry("scene", "betrayal", "Treachery, broken trust, or a revealed double-cross.", "treachery", "double cross"),
                    _entry("scene", "revelation", "A truth, secret, identity, or major fact is revealed.", "truth unveiled", "secret revealed"),
                    _entry("scene", "victory", "Success, conquest, achievement, or winning aftermath.", "success", "winning"),
                    _entry("scene", "defeat", "Loss, surrender, collapse, or failed effort.", "surrender", "loss"),
                    _entry("scene", "mourning", "Funeral, grieving, remembrance, or communal loss.", "funeral", "grieving"),
                    _entry("scene", "festival", "Public fair, carnival, holiday, or communal celebration.", "public celebration", "carnival"),
                    _entry("scene", "march", "Military march, procession, parade, or organized advance.", "marching", "procession", "parade"),
                    _entry("scene", "rescue", "Saving, extraction, recovery, or bringing someone to safety.", "extraction", "saving"),
                    _entry("scene", "training", "Practice, drills, lessons, sparring, or skill development.", "practice", "drills"),
                    _entry("scene", "storytelling", "Tale, narration, oral history, or recounting events.", "narration", "tale"),
                ],
            ),
            TagVocabularyGroup(
                key="mood",
                label="Mood",
                description="The emotional tone the music supports.",
                tags=[
                    _entry(
                        "mood",
                        "festive",
                        "Revelrous, jubilant, celebratory, or holiday-like tone.",
                        "revelry",
                        "jubilant",
                        "celebratory",
                        context_cues=(
                            "dance",
                            "dances",
                            "dancing",
                            "jig",
                            "feast",
                            "banquet",
                            "festival",
                        ),
                    ),
                    _entry(
                        "mood",
                        "heroic",
                        "Courageous, valorous, noble, or larger-than-life resolve.",
                        "courageous",
                        "valorous",
                        "noble resolve",
                        context_cues=(
                            "crown guard",
                            "royal guard",
                            "royal procession",
                            "heroic march",
                        ),
                    ),
                    _entry("mood", "mysterious", "Secretive, enigmatic, curious, or unexplained atmosphere.", "enigmatic", "secretive"),
                    _entry("mood", "tense", "Pressure, suspense, strain, or expectation of danger.", "suspenseful", "pressured"),
                    _entry("mood", "dark", "Bleak, sinister, morally shadowed, or oppressive tone.", "bleak", "sinister", "oppressive"),
                    _entry(
                        "mood",
                        "calm",
                        "Peaceful, settled, gentle, or emotionally untroubled tone.",
                        "peaceful",
                        "tranquil",
                        "relaxed",
                        context_cues=("quiet", "lullaby", "gentle", "stillness", "rest"),
                    ),
                    _entry(
                        "mood",
                        "eerie",
                        "Uncanny, ghostly, strange, or quietly unsettling tone.",
                        "uncanny",
                        "haunting",
                        "ghostly",
                        context_cues=(
                            "whisper",
                            "whispers",
                            "crypt",
                            "catacomb",
                            "catacombs",
                            "haunted",
                        ),
                    ),
                    _entry("mood", "melancholy", "Wistful sadness, loss, reflection, or subdued grief.", "sad", "wistful", "sorrowful"),
                    _entry(
                        "mood",
                        "romantic",
                        "Tenderness, intimacy, affection, or love-associated warmth.",
                        "romance",
                        "love theme",
                        "tender",
                        "affectionate",
                    ),
                    _entry("mood", "joyful", "Clear happiness, delight, cheer, or uncomplicated pleasure.", "cheerful", "happy", "delighted"),
                    _entry("mood", "hopeful", "Optimism, possibility, reassurance, or expectation of improvement.", "optimistic", "promising"),
                    _entry("mood", "uplifting", "Inspiring, encouraging, emotionally rising, or affirming.", "inspiring", "encouraging"),
                    _entry("mood", "playful", "Mischievous, lighthearted, teasing, or energetic fun.", "mischievous", "lighthearted"),
                    _entry("mood", "whimsical", "Fanciful, quirky, charmingly odd, or storybook-like.", "quirky", "fanciful"),
                    _entry("mood", "wondrous", "Awe, amazement, grandeur of discovery, or childlike wonder.", "awe inspiring", "wonder filled"),
                    _entry("mood", "ethereal", "Airy, weightless, otherworldly, or delicately unreal.", "otherworldly", "airy"),
                    _entry("mood", "majestic", "Grand, regal, stately, or imposing in scale and dignity.", "grand", "regal"),
                    _entry("mood", "solemn", "Serious, dignified, restrained, or weighty without necessarily being sad.", "dignified", "serious"),
                    _entry("mood", "sacred", "Holy, reverent, devotional, or spiritually elevated.", "holy", "reverent"),
                    _entry("mood", "ominous", "Foreboding, threatening, or signaling that something bad approaches.", "foreboding", "threatening"),
                    _entry("mood", "anxious", "Nervous, uneasy, unsettled, or worried anticipation.", "nervous", "uneasy"),
                    _entry("mood", "fearful", "Afraid, terrified, panicked, or directly frightened.", "afraid", "terrified"),
                    _entry("mood", "aggressive", "Hostile, furious, forceful, or confrontational energy.", "furious", "hostile"),
                    _entry("mood", "desperate", "Frantic, hopeless, cornered, or driven by urgent need.", "frantic", "hopeless"),
                    _entry("mood", "determined", "Resolute, focused, disciplined, or committed to a goal.", "resolute", "focused"),
                    _entry("mood", "defiant", "Rebellious, unyielding, resistant, or refusing submission.", "rebellious", "unyielding"),
                    _entry("mood", "adventurous", "Bold, questing, eager for risk, travel, or discovery.", "bold", "questing"),
                    _entry("mood", "curious", "Inquisitive, searching, attentive, or drawn toward an unknown answer.", "inquisitive", "searching"),
                    _entry("mood", "dreamy", "Dreamlike, hazy, drifting, or softly detached from reality.", "dreamlike", "hazy"),
                    _entry("mood", "nostalgic", "Remembrance, homesickness, or longing for an earlier time.", "remembrance", "homesick"),
                    _entry("mood", "lonely", "Isolated, solitary, abandoned, or emotionally alone.", "isolated", "solitary"),
                    _entry("mood", "bittersweet", "Happiness and sadness held together without resolving either.", "mixed emotions", "happy sad"),
                    _entry("mood", "warm", "Cozy, comforting, welcoming, or emotionally close.", "cozy", "comforting"),
                    _entry("mood", "cold", "Icy, detached, stark, or emotionally distant.", "icy", "detached"),
                    _entry("mood", "chaotic", "Disorderly, uncontrolled, unstable, or rapidly shifting.", "disorderly", "uncontrolled"),
                    _entry("mood", "urgent", "Hurried, time-critical, pressing, or demanding immediate action.", "hurried", "time critical"),
                    _entry("mood", "triumphant", "Victorious, glorious, exultant, or celebrating hard-won success.", "victorious", "glorious"),
                    _entry("mood", "humorous", "Funny, comic, absurd, or intentionally amusing.", "funny", "comic"),
                    _entry("mood", "magical", "Enchanted, arcane, spellbound, or filled with overt magic.", "enchanted", "arcane"),
                ],
            ),
        ]
    )


def _document_json(document: TagVocabularyDocument) -> str:
    return json.dumps(
        document.model_dump(mode="json"),
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    )


def vocabulary_fingerprint(document: TagVocabularyDocument) -> str:
    return hashlib.sha256(_document_json(document).encode("utf-8")).hexdigest()


def default_tag_vocabulary_snapshot() -> TagVocabularySnapshot:
    document = default_tag_vocabulary()
    return TagVocabularySnapshot(
        document=document,
        revision=1,
        fingerprint=vocabulary_fingerprint(document),
    )


def _merge_seed_vocabulary(
    current: TagVocabularyDocument,
    seed: TagVocabularyDocument,
) -> TagVocabularyDocument:
    """Add new recommended tags without replacing operator-owned vocabulary data."""

    groups = [group.model_copy(deep=True) for group in current.groups]
    groups_by_key = {group.key: group for group in groups}
    entries = [tag for group in groups for tag in group.tags]
    locations_by_id = {
        tag.id: (group, index, tag)
        for group in groups
        for index, tag in enumerate(group.tags)
    }
    used_names = {tag.name for tag in entries}
    used_aliases = {alias for tag in entries for alias in tag.aliases}

    for seed_group in seed.groups:
        target_group = groups_by_key.get(seed_group.key)
        if target_group is None:
            if len(groups) >= 20:
                continue
            target_group = seed_group.model_copy(update={"tags": []}, deep=True)
            groups.append(target_group)
            groups_by_key[target_group.key] = target_group

        for seed_tag in seed_group.tags:
            if len(entries) >= MAX_VOCABULARY_TAGS or len(target_group.tags) >= 100:
                break
            existing = locations_by_id.get(seed_tag.id)
            if existing is not None:
                existing_group, existing_index, existing_tag = existing
                if existing_tag.name == seed_tag.name:
                    context_cues = list(
                        dict.fromkeys(
                            [*existing_tag.context_cues, *seed_tag.context_cues]
                        )
                    )
                    if context_cues != existing_tag.context_cues:
                        updated = existing_tag.model_copy(
                            update={"context_cues": context_cues}
                        )
                        existing_group.tags[existing_index] = updated
                        locations_by_id[updated.id] = (
                            existing_group,
                            existing_index,
                            updated,
                        )
                continue
            if (
                seed_tag.name in used_names
                or seed_tag.name in used_aliases
            ):
                continue

            aliases = [
                alias
                for alias in seed_tag.aliases
                if alias not in used_names and alias not in used_aliases
            ]
            added = seed_tag.model_copy(update={"aliases": aliases})
            target_group.tags.append(added)
            entries.append(added)
            used_names.add(added.name)
            used_aliases.update(added.aliases)
            locations_by_id[added.id] = (
                target_group,
                len(target_group.tags) - 1,
                added,
            )

    return TagVocabularyDocument(groups=groups)


def load_tag_vocabulary(db: Session) -> TagVocabularySnapshot:
    row = db.get(AssistantTagVocabulary, TAG_VOCABULARY_KEY)
    if row is None:
        document = default_tag_vocabulary()
        row = AssistantTagVocabulary(
            key=TAG_VOCABULARY_KEY,
            revision=1,
            seed_version=TAG_VOCABULARY_SEED_VERSION,
            document_json=_document_json(document),
        )
        db.add(row)
        db.commit()
        db.refresh(row)
    document = TagVocabularyDocument.model_validate_json(row.document_json)
    if row.seed_version < TAG_VOCABULARY_SEED_VERSION:
        merged = _merge_seed_vocabulary(document, default_tag_vocabulary())
        if merged != document:
            row.document_json = _document_json(merged)
            row.revision += 1
            document = merged
        row.seed_version = TAG_VOCABULARY_SEED_VERSION
        db.commit()
        db.refresh(row)
    return TagVocabularySnapshot(
        document=document,
        revision=row.revision,
        fingerprint=vocabulary_fingerprint(document),
    )


def replace_tag_vocabulary(
    db: Session,
    *,
    expected_revision: int,
    document: TagVocabularyDocument,
) -> TagVocabularySnapshot:
    current = load_tag_vocabulary(db)
    if current.revision != expected_revision:
        raise TagVocabularyConflictError(
            "The tag vocabulary changed after this page was loaded. Reload it and try again."
        )
    result = cast(
        "CursorResult[Any]",
        db.execute(
            update(AssistantTagVocabulary)
            .where(
                AssistantTagVocabulary.key == TAG_VOCABULARY_KEY,
                AssistantTagVocabulary.revision == expected_revision,
            )
            .values(
                document_json=_document_json(document),
                revision=expected_revision + 1,
                updated_at=utcnow(),
            )
        ),
    )
    if result.rowcount != 1:
        db.rollback()
        raise TagVocabularyConflictError(
            "The tag vocabulary changed while it was being saved. Reload it and try again."
        )
    db.commit()
    return TagVocabularySnapshot(
        document=document,
        revision=expected_revision + 1,
        fingerprint=vocabulary_fingerprint(document),
    )
