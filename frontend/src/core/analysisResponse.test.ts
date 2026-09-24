import { describe, expect, it } from "vitest";
import type { LibraryTagPage, LibraryTagTrack } from "./api";
import { requireCurrentTagPage, requireCurrentTagTrack } from "./analysisResponse";

const suggestion = {tag:"calm",analyzer_id:"model-context-tagger/v8",source_signature:"a".repeat(64),support:"tentative",evidence:["The quiet opening may suit this use."],evidence_ids:["audio.sections.s1"],contradiction_ids:["audio.sections.s3"],status:"pending"};
const track = {track_id:1,path:"song.flac",title:"Song",display_title:"Song",artist:"",album:"",manual_tags:[],analysis_tags:["calm"],analysis_suggestions:[suggestion]} as LibraryTagTrack;
const page = {items:[track],total:1,offset:0,limit:50} as LibraryTagPage;

describe("current song analysis responses", () => {
  it("preserves per-tag support and abstention without changing authored tags", () => {
    expect(requireCurrentTagPage(page)).toBe(page);
    const abstention = {...track,manual_tags:["authored"],analysis_tags:[],analysis_suggestions:[]};
    expect(requireCurrentTagTrack(abstention)).toBe(abstention);
  });
  it.each([
    {...suggestion,support:undefined,confidence:"high"},
    {...suggestion,support:"high"},
    {...suggestion,analyzer_id:"model-context-tagger/v7"},
    {...suggestion,evidence_ids:[]},
    {...suggestion,evidence:["x".repeat(513)]},
    {...suggestion,contradiction_ids:undefined},
  ])("rejects old or malformed decisions before display", (invalid) => {
    expect(() => requireCurrentTagTrack({...track,analysis_suggestions:[invalid]} as unknown as LibraryTagTrack)).toThrow(/does not match/);
  });
  it("rejects removed whole-track fields and invalid page shapes", () => {
    for (const field of ["analysis_analyzer","analysis_confidence","audio_signal"]) {
      expect(() => requireCurrentTagPage({...page,items:[{...track,[field]:null}]})).toThrow(/does not match/);
    }
    expect(() => requireCurrentTagPage({items:null} as unknown as LibraryTagPage)).toThrow(/does not match/);
  });
});
