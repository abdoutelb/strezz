# stressTest
stress test for any website

TO RUN the solution replace `URL` with your site
*)- use `URL=URL node index.js`

*) `docker build -t stress-web --build-arg SITE_URL=URL .`

*) `docker run -itd --name stress-web` 

check your `localhost:40000`
