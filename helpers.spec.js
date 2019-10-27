const helpers = require("./helpers");

const isArabic = helpers.isArabic;

describe("isArabic", function() {
  it("should be defined", function() {
    expect(isArabic).toBeDefined();
  });
  it("should be a function", function() {
    expect(isArabic).toBeInstanceOf(Function);
  });
});
