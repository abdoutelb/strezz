const helpers = require("./helpers");


describe("extract urls from url", function () {
  it("should be defined", function () {
    expect(helpers.extractUrls).toBeDefined();
  });
  it("should be a function", function () {
    expect(helpers.extractUrls).toBeInstanceOf(Function);
  });
});

describe("read python file", function () {
  it("should be defined", function () {
    expect(helpers.extractFile).toBeDefined();
  });
  it("should be a function", function () {
    expect(helpers.extractFile).toBeInstanceOf(Function);
  });
});

describe("generate tasks for locust", function () {
  it("should be defined", function () {
    expect(helpers.generateTasks).toBeDefined();
  });
  it("should be a function", function () {
    expect(helpers.generateTasks).toBeInstanceOf(Function);
  });
});

describe("generate locust file", function () {
  it("should be defined", function () {
    expect(helpers.generateFile).toBeDefined();
  });
  it("should be a function", function () {
    expect(helpers.generateFile).toBeInstanceOf(Function);
  });
});
