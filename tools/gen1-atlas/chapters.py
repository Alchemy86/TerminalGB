"""Which chapter of the atlas an address belongs to.

pokered's own section names are an assembler's grouping — "WRAM", "Main Data",
"bank1" — and several of them mix half a dozen unrelated subsystems because a
linker section is about *where bytes fit*, not about what they mean.  The atlas
groups by subject instead, so a reader looking for "the boxes" or "the bag" finds
one chapter rather than a range.

That grouping is our editorial judgement, and it is written here as rules rather
than typed into 2,886 spreadsheet cells: a rule can be reviewed, argued with, and
re-run when pokered moves.  Rules are tried in order and the first match wins, so
a specific symbol name can always override a range.

`SYMBOL_RULES` are matched against the symbol with `re.search`; `RANGE_RULES` are
matched against (region, address).  Everything unmatched falls back to the
section-derived chapter in `SECTION_RULES`, and anything still unmatched is
`misc`, which the renderer reports so it can be given a home.
"""

from __future__ import annotations

import re

# The chapters, in the order the atlas presents them, with the one-line summary
# that heads each chapter page.  Adding a chapter means adding it here.
CHAPTERS: dict[str, str] = {
    "player": "Who you are: name, ID, badges, money, options, play time.",
    "party": "The six Pokemon you carry, and the 44-byte structure they are made of.",
    "storage": "The PC: twelve boxes, the one that is live, and where the other eleven sit.",
    "battle": "Everything that exists only while a battle is running.",
    "bag": "Items, the PC item store, and the two coin purses.",
    "pokedex": "Owned and seen, two flag arrays and nothing else.",
    "events": "The story: 2,560 event flags, the missable-object list, and per-map scripts.",
    "overworld": "The map you are standing on: header, blocks, warps, signs, connections.",
    "sprites": "The people and objects on screen, and the engine state that walks them.",
    "screen": "The tilemap that reads back as text, the text engine, and the menu cursor.",
    "graphics": "VRAM, the tile blocks that reach it, and Gen 1's sprite compression.",
    "audio": "The sound engine's channel state.",
    "link": "The cable: the serial exchange buffers and the trade and battle state.",
    "rng": "The dice, and why they are not reproducible.",
    "save": "Cartridge RAM: what a `.sav` is made of, and the checksums over it.",
    "system": "Below the game: the stack, the joypad, OAM DMA, and the frame counter.",
    "scratch": "The union area: bytes a dozen unrelated routines take turns owning.",
    "rom-data": "Tables in the cartridge itself: species, moves, items, trainers, graphics.",
    "misc": "Not yet given a home.  Every entry here is an admission, not a category.",
}

# Tried first, in order.  `re.search`, so anchor deliberately.
SYMBOL_RULES: list[tuple[str, str]] = [
    # --- things whose name would otherwise drag them into the wrong chapter ---
    (r"^wBoxItems|^wNumBoxItems", "bag"),          # the PC *item* store, not a Pokemon box
    (r"^wBoxMonNicks$|^wBoxMonOT$", "storage"),
    (r"^wLinkBattleRandomNumberList", "rng"),
    (r"^wTrainerHeader|^wTrainerScreen", "events"),
    (r"^wCardKeyDoor|^wCompletedInGameTradeFlags", "events"),

    # --- player ---
    (r"^wPlayerName$|^wPlayerID$|^wRivalName$|^wPlayerMoney$|^wPlayerCoins$", "player"),
    (r"^wObtainedBadges|^wUnusedObtainedBadges|^wTempObtainedBadges", "player"),
    (r"^wOptions|^wLetterPrintingDelayFlags|^wPlayTime|^wNumHoFTeams", "player"),
    (r"^wPlayerGender", "player"),

    # --- storage (the PC boxes, and the Day Care's single lodger) ---
    (r"^wDayCare", "storage"),
    (r"^wNumSafariBalls", "bag"),
    (r"^wLinkEnemyTrainerName", "link"),
    (r"^w[XY]OffsetSinceLastSpecialWarp", "overworld"),

    (r"^wBoxCount$|^wBoxSpecies|^wBoxMon|^wBoxDataStart|^wBoxDataEnd", "storage"),
    (r"^wCurrentBoxNum|^wNumInBox|^wBoxNumString|^wWhichPokemon$", "storage"),
    (r"^sBox\d|^sBoxMon|^sBoxCount|^sBank\dAllBoxes|^sBank\dIndividualBoxChecksums",
     "storage"),

    # --- party ---
    (r"^wPartyCount$|^wPartySpecies|^wPartyMon|^wPartyData|^wPartyMonOT|^wPartyMonNicks",
     "party"),
    (r"^wPartyFoughtCurrentEnemyFlags|^wPartyGainExpFlags|^wPartyAndBillsPC", "party"),
    (r"^sPartyData|^sPartyDataEnd", "party"),

    # --- battle ---
    (r"^wEnemyMon|^wBattleMon|^wEnemyParty|^wEnemyMons|^wPlayerMon", "battle"),
    (r"^wIsInBattle|^wBattleType|^wCurOpponent|^wTrainerNo|^wCurEnemyLevel", "battle"),
    (r"^wBattle|^wCriticalHit|^wDamage|^wAI|^wEngagedTrainer|^wLoneAttack|^wGymLeaderNo",
     "battle"),
    (r"^wPlayerSelectedMove|^wEnemySelectedMove|^wPlayerUsedMove|^wEnemyUsedMove", "battle"),
    (r"^wPlayerDisabledMove|^wEnemyDisabledMove|^wPlayerNumAttacksLeft|^wEnemyNumAttacks",
     "battle"),
    (r"^wPlayerSubstituteHP|^wEnemySubstituteHP|^wPlayerBattleStatus|^wEnemyBattleStatus",
     "battle"),
    (r"^wSafari|^wEscapedFromBattle|^wMonIsDisobedient|^wMoveDidntMiss|^wMoveMenuType",
     "battle"),
    (r"^wTransformedEnemyMon|^wLowHealthAlarm|^wTotalPayDayMoney|^wExpAmountGained",
     "battle"),
    (r"^wInHandlePlayerMonFainted|^wCanEvolveFlags|^wForceEvolution|^wEvoStoneItemID",
     "battle"),
    (r"^wBoostExpByExpAll|^wLastSwitchInEnemyMonHP|^wNumRunAttempts", "battle"),
    (r"^wAnimation|^wAnimPalette|^wAnimSound|^wSubAnimation|^wBaseCoord|^wFB", "battle"),
    (r"^wCurrentMove|^wMoveMissed|^wTypeEffectiveness|^wAttack|^wDefense", "battle"),
    (r"^wHPBar|^wDamageMultipliers|^wUnusedD119", "battle"),

    # --- bag / money ---
    (r"^wNumBagItems|^wBagItems|^wBagSavedMenuItem|^wFilteredBagItems", "bag"),
    (r"^wItemPrice|^wItemQuantity|^wCurItem|^wcf91$|^wPseudoItemID", "bag"),
    (r"^wTempCoins|^wPayoutCoins|^wPrize\d", "bag"),

    # --- pokedex ---
    (r"^wPokedex|^wDexRating|^wPokedexNum", "pokedex"),

    # --- events / scripts ---
    (r"^wEventFlags|^wObtainedHiddenItemsFlags|^wObtainedHiddenCoinsFlags", "events"),
    (r"CurScript$|^wCurMapScript|^wScriptedNPC|^wNPCMovementScript", "events"),
    (r"^wToggleableObject|^wSavedSpriteImageIndex|^wOpponentAfterWrongAnswer", "events"),
    (r"^wBeatGymFlags|^wDefeated|^wGameProgressFlags|^wSSAnne", "events"),

    # --- overworld / map ---
    (r"^wCurMap|^wLastMap|^wCurrentMap|^wYCoord|^wXCoord|^wYBlockCoord|^wXBlockCoord",
     "overworld"),
    (r"Connect(ed|ion)|^wNorth|^wSouth|^wEast|^wWest", "overworld"),
    (r"^wWarp|^wNumberOfWarps|^wDestinationWarp|^wSign|^wNumSigns", "overworld"),
    (r"^wTileset|^wGrassTile|^wGrassRate|^wWaterRate|^wGrassMons|^wWaterMons",
     "overworld"),
    (r"^wMapBackgroundTile|^wMapView|^wMapMusic|^wMapPalOffset|^wMapSprite", "overworld"),
    (r"^wOverworldMap|^wCurrentTileBlockMapViewPointer|^wRedrawRowOrColumn", "overworld"),
    (r"^wPlayerMovingDirection|^wPlayerLastStopDirection|^wPlayerDirection", "overworld"),
    (r"^wWalkCounter|^wWalkBikeSurf|^wStepCounter|^wNumSteps|^wSpriteSet", "overworld"),
    (r"^wTilePlayerStandingOn|^wTileInFrontOfPlayer|^wCoord|^wcf0d", "overworld"),
    (r"^wDungeonWarp|^wLastBlackoutMap|^wDefaultMap|^wUsedDungeonWarp", "overworld"),
    (r"^wOffsetSinceLastSpecialWarp|^wD732|^wD733|^wD72[0-9A-E]|^wD73[0-9A-F]",
     "overworld"),

    # --- sprites ---
    (r"^wSprite\d|^wSpriteStateData|^wSpritePlayerStateData|^wNumSprites", "sprites"),
    (r"^wOAMBuffer|^wShadowOAM|^wOAM|^wSpriteIndex|^wSpriteScreen|^wSpriteAnim",
     "sprites"),

    # --- screen / text / menus ---
    (r"^wTileMap|^wTileMapBackup|^wTopMenuItem|^wCurrentMenuItem|^wMaxMenuItem", "screen"),
    (r"^wMenu|^wLastMenuItem|^wListScrollOffset|^wListCount|^wListPointer|^wListMenu",
     "screen"),
    (r"^wTextDest|^wTextBox|^wTextPredef|^wDoNotWaitForButtonPress|^wLetterDelay",
     "screen"),
    (r"^wCursor|^wTileBehindCursor|^wStringBuffer|^wcd6d$|^wNameBuffer", "screen"),
    (r"^wUnusedC000|^wFontLoaded|^wPrinter|^wScreenEdge|^wBGMap|^wBaseCoord", "screen"),

    # --- graphics ---
    (r"^v[A-Z]", "graphics"),
    (r"^sSpriteBuffer|^wSpriteInputPtr|^wSpriteOutputPtr|^wSpriteCurPosX", "graphics"),
    (r"^wSpriteLoad|^wSpriteUnpack|^wSpriteFlipped|^wSpriteInputBit|^wSpriteOutputBit",
     "graphics"),
    (r"^wSpriteDecodeTable|^wSpriteColumn|^wSpriteHeight|^wSpriteWidth", "graphics"),
    (r"^wMonPalette|^wPalPacket|^wPartyMenuBlk|^wSGB|^wCopyData|^wTileSetTile",
     "graphics"),

    # --- audio ---
    (r"^wChannel|^wMusic|^wSfx|^wSound|^wAudio|^wStereoPanning|^wSavedVolume",
     "audio"),
    (r"^wMute|^wDisableChannelOutput|^wLowHealthAlarm|^wNewSoundID|^wLastMusicSoundID",
     "audio"),

    # --- link ---
    (r"^wSerial|^wLinkState|^wLinkMenu|^wLinkTimeout|^wTradeC|^wTrade|^wPlayerBlackout",
     "link"),
    (r"^hSerial", "link"),

    # --- rng ---
    (r"^hRandom", "rng"),

    # --- save ---
    (r"^sMainData|^sSpriteData|^sTileAnimations|^sCurBox|^sPlayerName|^sHallOfFame",
     "save"),
    (r"^sGameData|^sTrainerName|^sChecksum|CheckSum$", "save"),

    # --- story state that lives in the saved block ---
    (r"^wStatusFlags\d|^wElite4Flags|^wMovementFlags|^wTownVisitedFlag", "events"),
    (r"^wFossilItem|^wFossilMon|^wRivalStarter|^wPlayerStarter|^wBoulderSprite", "events"),
    (r"^wLockTrashCan|^wFirstLockTrashCan|^wSecondLockTrashCan|^wSafariSteps", "events"),
    (r"^wGivenMon|^wSecondLock|^wUnusedCardKeyGateID|^wWhichDungeonWarp", "events"),
    (r"^wMainDataStart|^wMainDataEnd|^wGameProgress", "save"),
    (r"^wDestinationMap|^wCableClubDestinationMap|^wPlayerJumping", "overworld"),
    (r"^wObjectDataPointerTemp|^wOffsetSinceLastSpecialWarp|^wUnusedLastMapWidth",
     "overworld"),
    (r"^wUnusedMapVariable|^wUnusedPlayerDataByte|^wTileInFrontOfBoulder", "overworld"),
    (r"^wPredef", "system"),

    # --- system ---
    (r"^wStack|^hJoy|^hVBlank|^hDMA|^hSCX|^hSCY|^hTilesetType|^hClear", "system"),
    (r"^wJoy|^wSimulatedJoypad|^wOverrideSimulatedJoypad|^wSaved.*Joypad", "system"),
    (r"^hFrameCounter|^hQuotient|^hDivisor|^hMultipl|^hProduct|^hRemainder", "system"),
    (r"^wUpdateSpritesEnabled|^wLCD|^wIsKeyItem|^hSoftReset|^hLoadedROMBank", "system"),
]

# Falls through to here when no symbol rule matched.
RANGE_RULES: list[tuple[str, int, int, str]] = [
    ("VRAM", 0x8000, 0x9FFF, "graphics"),
    ("HRAM", 0xFF80, 0xFFFE, "system"),
    ("WRAM0", 0xC000, 0xC0FF, "audio"),
    ("WRAM0", 0xC100, 0xC2FF, "sprites"),
    ("WRAM0", 0xC300, 0xC39F, "sprites"),
    ("WRAM0", 0xC3A0, 0xC6E7, "screen"),
    ("WRAM0", 0xC6E8, 0xCBFB, "overworld"),
    ("WRAM0", 0xCBFC, 0xD162, "scratch"),
    ("WRAM0", 0xD163, 0xD2F6, "party"),
    ("WRAM0", 0xDA80, 0xDEE1, "storage"),
    ("WRAM0", 0xDF00, 0xDFFF, "system"),
]

# Last resort: pokered's own section name.
SECTION_RULES: dict[str, str] = {
    "(unallocated)": "system",
    "Audio RAM": "audio",
    "Sprite State Data": "sprites",
    "OAM Buffer": "sprites",
    "Tilemap": "screen",
    "Overworld Map": "overworld",
    "Party Data": "party",
    "Current Box Data": "storage",
    "Stack": "system",
    "Sprite Buffers": "graphics",
    "Save Data": "save",
    "Saved Boxes 1": "storage",
    "Saved Boxes 2": "storage",
    "VRAM": "graphics",
    "OAM DMA": "system",
    "HRAM": "system",
}

_COMPILED = [(re.compile(pat), chapter) for pat, chapter in SYMBOL_RULES]


def classify(region: str, addr: int, symbol: str, section: str) -> str:
    """Return the chapter for one atlas entry.  Never raises; worst case 'misc'."""
    for rx, chapter in _COMPILED:
        if rx.search(symbol):
            return chapter
    for reg, lo, hi, chapter in RANGE_RULES:
        if region == reg and lo <= addr <= hi:
            return chapter
    if region in ("ROM0", "ROMX"):
        return "rom-data"
    return SECTION_RULES.get(section, "misc")
